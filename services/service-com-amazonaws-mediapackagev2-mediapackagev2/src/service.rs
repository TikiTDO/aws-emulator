// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-mediapackagev2-mediapackagev2` service: how it claims requests and how it words the
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
//!   correctly. THIS service uses it: it puts the input-validation gate described
//!   below in front of every modelled operation and lets the generated routing table
//!   serve the request from there.
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
//! ## The data plane
//!
//! MediaPackage publishes two kinds of address. The CONTROL plane is
//! `mediapackagev2.<region>.amazonaws.com`, whose first label names the service, and
//! that is what the framework's route key resolves. The DATA plane is what this service
//! ANNOUNCES for a client to dial later — `EgressDomain` on a channel group, and
//! `IngestEndpoints[].Url` on a channel, plus the `Url` of every manifest an origin
//! endpoint publishes. AWS serves those on per-group hostnames whose first label is an
//! opaque digest; a client on the machine this emulator runs on resolves none of them, so
//! the address announced is the endpoint the creating request reached, with everything the
//! hostname carried moved into the path
//! ([`crate::state::announced_data_plane_domain`]). Either way, a request to the announced
//! address has to reach THIS service rather than the control-plane resolver, which cannot
//! name a service from an opaque label or from the emulator's own address and answers
//! `404 UnknownService`.
//!
//! Three seams carry it, and nothing else changes:
//!
//! - [`Service::can_handle`] also claims a request addressed to either of those
//!   addressings ([`crate::state::addressed_data_plane`]), which is what routes it here at
//!   all.
//! - [`Service::override_routing_error`] serves the request that presents NO credential.
//!   A data-plane request names no modelled operation by construction (`/out/v1/…`,
//!   `/in/v1/…` are not in the model), so the router reaches exactly this seam for it —
//!   past the input-validation gate below, which only fronts the modelled operations —
//!   and reaches it BEFORE authentication, which is what lets an anonymous player be
//!   answered at all.
//! - [`Service::override_operation`] serves the request that DOES present a SigV4
//!   credential, through [`CredentialedDataPlane`]. The router authenticates whatever it
//!   names an operation for and skips authentication for everything it answers from the
//!   routing-error seam, so naming an operation here is what puts the emulator's own
//!   signature verification in front of a credentialed push or fetch to a published URL —
//!   the same verification a control-plane request gets, rather than a second signature
//!   check written into this service. A credential the emulator cannot verify is therefore
//!   refused by the router (`403`, with its signature-mismatch wording), which is a
//!   DIFFERENT refusal from the access-denied `403` a request carrying no credential at
//!   all gets below.
//!
//! What the data plane enforces:
//!
//! - EGRESS (`GET`/`HEAD /out/v1/<group>/<channel>/<endpoint>/<object>`) is gated by the
//!   origin endpoint's resource policy, against the action that method needs
//!   (`mediapackagev2:GetObject` for a `GET`, `mediapackagev2:GetHeadObject` for a
//!   `HEAD`). With no policy granting it to the `*` principal the fetch is `403` with an
//!   access-denied body; with the grant in
//!   place the fetch is authorized and the object is looked up, which answers `404` while
//!   no content has been packaged for it. Those are the two states a player branches on,
//!   and the resource policy is the only grant this emulator can evaluate: it performs no
//!   IAM identity-policy evaluation, so an authorization that would come from a caller's
//!   identity policy is not one it can honour.
//! - INGEST (`PUT`/`POST /in/v1/<group>/<index>/<channel>/<object>`) requires a SigV4
//!   credential: MediaPackage returns no credential for ingest, so an unauthenticated push
//!   is `403` with an access-denied body. Whether a credential that IS present carries a
//!   valid signature is the emulator's own business — the emulator verifies every request
//!   signature and a service never checks signatures itself — so this handler does not
//!   re-verify it; the credentialed push is routed as an operation instead
//!   ([`CredentialedDataPlane`]) so the router's authenticator judges that signature
//!   first, and a push that reaches this handler is one the emulator accepted. It is then
//!   recorded against the channel, within the three
//!   bounds the store declares: the per-object size, the per-channel object count, and the
//!   store-wide retained bytes (`state::MAX_INGESTED_BYTES`), since the channel count is
//!   itself caller-driven. What a channel retains is released by resetting the channel,
//!   which removes its previous content
//!   ([`crate::state::Channel::discard_ingested_content`]), or by deleting it.
//!
//! Both halves resolve the group, channel and endpoint their path names in the REGION the
//! announced address names ([`crate::state::addressed_data_plane`]): the data plane is
//! regional exactly as the control plane is, and a data-plane request carries no
//! credential scope to read a Region from, so the URL this service handed the client is
//! what says which Region's resources it reaches — a label of the published hostname, or a
//! path segment of the address on this emulator's own endpoint.
//!
//! ## Why this file validates input
//!
//! MediaPackage's `ValidationException` is modelled with TWO members — `Message` and
//! `ValidationExceptionType` — and AWS sets both on every 400. The framework's
//! pre-dispatch envelope cannot: it renders a refusal as a generic
//! `{"__type":"ValidationException","message":...}` with no modelled member at all,
//! because at that point the refusal is a protocol-level fact rather than one of this
//! service's error SHAPES, and the only seam over it ([`Service::override_refusal`])
//! chooses a code and a sentence, not a body.
//!
//! So this service answers a constraint refusal from its own modelled shape, through
//! [`ModeledInputValidation`]: a gate [`Service::override_operation`] puts in front of
//! every modelled operation, which asks the framework's OWN constraint walk
//! (`aws_smithy_schema_server::constraints::validate`, the same call the centralized
//! sequence makes) what the request violates, renders the first violation through
//! [`crate::state::member_validation_failed`] (the one home of this service's refusal
//! bodies, beside the wording every other refusal uses), and otherwise hands the request
//! straight to the generated operation. The rules enforced are therefore exactly the
//! model's — nothing about a constraint is restated here — and only the BODY of the
//! refusal changes.
//!
//! Two things the gate deliberately does not touch: authentication, which the router
//! settles before any operation runs and reads off the MODELLED operation the routing
//! key names (so a gate cannot weaken an operation's auth schemes), and the
//! per-operation telemetry the centralized sequence records — a request the gate
//! refuses never enters that sequence, so it is counted nowhere, exactly as a request
//! refused before dispatch is.
//!
//! ## How a response with no body is framed
//!
//! Two operations here answer with no body at all: `TagResource` (POST `/tags/{ResourceArn}`)
//! and `UntagResource` (DELETE `/tags/{ResourceArn}`) are both modelled with output `Unit`
//! at HTTP **204**
//! (`smithy_query(query="operation",operation="TagResource,UntagResource",model_path="services/service-com-amazonaws-mediapackagev2-mediapackagev2/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json")`).
//! A response with no body still has a LENGTH, and AWS states it: these answers carry
//! `content-length: 0`. The protocol's serializer leaves the header off, which leaves the
//! length to the transport to frame — so a client, proxy or SDK transport that keys on a
//! framing header being present on a bodiless response reads the emulator's reply
//! differently from the service's.
//!
//! [`ServerProtocol`] is therefore implemented on the service itself and served through
//! [`Service::override_protocol`]: the model's own protocol in every respect except one —
//! a response at the no-content status carries no body and states the length it has, which
//! is the ONE reading of that rule this tree shares
//! ([`::service_common::no_content_responses::without_content`]), called rather than
//! copied. A response that carries content is left exactly as the model's protocol renders
//! it, framed from the body it carries. The data plane above frames its own bodiless answer
//! the same way, so the whole surface states the length of an answer that has none.
//!
//! An answer served over HTTP/1.1 reaches the client without it: the HTTP writer the
//! vendored transport uses removes `content-length` from a `204`, which RFC 7230 §3.3.2
//! forbids there, and that writer is below anything this crate decides. What this service
//! states is what the in-process front door delivers (the inter-service callback path and
//! the SDK connector both serve the router's response unchanged).

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{validate, Violation};
use aws_smithy_schema_server::{ProtocolError, ServerProtocol};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, Refusal,
    Refused, ServerHttpProtocol, Service,
};

/// `ValidationException`'s modelled status (`httpError` 400 on
/// `com.amazonaws.mediapackagev2#ValidationException`).
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// `ValidationException`'s wire error code — the shape name, which restJson1 puts in
/// `x-amzn-errortype`.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The content type the data plane answers its own (non-modelled) responses with.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The body of a data-plane refusal with no grant behind it.
///
/// One sentence, carrying nothing from the request: what a caller branches on is the
/// `403` and this text, and the service does not echo the resource it would not serve.
const ACCESS_DENIED_MESSAGE: &str = "Access denied";

/// The `com-amazonaws-mediapackagev2-mediapackagev2` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because the data plane is served from [`Service::override_routing_error`],
    /// which is not an operation and so has no `self.engine` of its own; it must read and
    /// write the one store the control-plane operations use, not a second copy.
    engine: Arc<Eng>,
    /// The data-plane handler the router names as an OPERATION for a request that presents
    /// a SigV4 credential, so the emulator authenticates it before anything is served.
    credentialed_data_plane: CredentialedDataPlane<Eng>,
    /// The input-validation gate for each modelled operation, keyed by the routing key
    /// the protocol resolves for it (the operation's own `NAME`). Built once, in `new`.
    input_validation: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            credentialed_data_plane: CredentialedDataPlane {
                engine: Arc::clone(&engine),
            },
            engine,
            input_validation: input_validation_gates::<Eng>(),
        }
    }
}

/// The auth schemes a credentialed data-plane request is verified under.
///
/// SigV4 is the only scheme this service's model declares (`awsService.sigv4Name`
/// `mediapackagev2`), and MediaPackage hands out no data-plane credential of another kind,
/// so a published URL is reached either anonymously — which the routing-error seam answers
/// without the router authenticating anything — or with a SigV4 credential, which this
/// names so the router's SigV4 authenticator verifies it.
const DATA_PLANE_AUTH_SCHEMES: &[&str] = &[::emulator_server::aws_credentials::SIGV4_SCHEME];

/// The data plane of a request that PRESENTS a SigV4 credential, as an [`ErasedOperation`]
/// so the router authenticates it.
///
/// It holds the SAME shared engine the generated entry threads into every operation, and
/// serves the request through exactly the functions the routing-error seam uses, so the
/// credentialed and the anonymous path cannot diverge in what they serve — only in whether
/// a signature was verified on the way in.
struct CredentialedDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for CredentialedDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match data_plane_of_request(req) {
                Some(addressed) => match addressed.plane {
                    crate::state::DataPlane::Egress => {
                        serve_egress(&*self.engine, req, &addressed)
                    }
                    crate::state::DataPlane::Ingest => {
                        serve_ingest(&*self.engine, req, &addressed)
                    }
                },
                // Unreachable: `override_operation` names this only for a request
                // addressed to one of the data-plane addresses this service publishes.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // This is NOT a modelled operation, so there is no modelled operation for the
        // router to read schemes off: without this the router would find no scheme for a
        // data-plane request and could not verify the credential it carries.
        Some(DATA_PLANE_AUTH_SCHEMES)
    }
}

/// One [`ModeledInputValidation`] gate per modelled operation.
///
/// Each gate is zero-sized: it carries only the operation TYPE, which is what names the
/// input shape and its schema. The key is the operation's own `NAME`, so it is the
/// model's routing key rather than a literal written here.
fn input_validation_gates<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut gates: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    macro_rules! gate {
        ($map:ident; $($module:ident :: $operation:ident),+ $(,)?) => {
            $(
                $map.insert(
                    <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                    Box::new(
                        ModeledInputValidation::<crate::ops::$module::$operation<Eng>>::new(),
                    ) as Box<dyn ErasedOperation>,
                );
            )+
        };
    }
    // Every operation the model binds. A constraint is a property of the INPUT SHAPE,
    // so the gate goes in front of all of them, implemented or still scaffolded: a
    // request that violates the model is refused the same way whoever serves it.
    gate!(
        gates;
        cancel_harvest_job::CancelHarvestJob,
        create_channel::CreateChannel,
        create_channel_group::CreateChannelGroup,
        create_harvest_job::CreateHarvestJob,
        create_origin_endpoint::CreateOriginEndpoint,
        delete_channel::DeleteChannel,
        delete_channel_group::DeleteChannelGroup,
        delete_channel_policy::DeleteChannelPolicy,
        delete_origin_endpoint::DeleteOriginEndpoint,
        delete_origin_endpoint_policy::DeleteOriginEndpointPolicy,
        get_channel::GetChannel,
        get_channel_group::GetChannelGroup,
        get_channel_policy::GetChannelPolicy,
        get_harvest_job::GetHarvestJob,
        get_origin_endpoint::GetOriginEndpoint,
        get_origin_endpoint_policy::GetOriginEndpointPolicy,
        list_channel_groups::ListChannelGroups,
        list_channels::ListChannels,
        list_harvest_jobs::ListHarvestJobs,
        list_origin_endpoints::ListOriginEndpoints,
        list_tags_for_resource::ListTagsForResource,
        put_channel_policy::PutChannelPolicy,
        put_origin_endpoint_policy::PutOriginEndpointPolicy,
        reset_channel_state::ResetChannelState,
        reset_origin_endpoint_state::ResetOriginEndpointState,
        tag_resource::TagResource,
        untag_resource::UntagResource,
        update_channel::UpdateChannel,
        update_channel_group::UpdateChannelGroup,
        update_origin_endpoint::UpdateOriginEndpoint,
    );
    gates
}

/// The modelled-`ValidationException` gate in front of one operation `Op`.
///
/// Zero-sized: `Op` is a type parameter only, naming the input shape and the schema the
/// constraint walk runs against. It holds no engine and no state, so the operation it
/// guards stays the single owner of the request's behaviour.
struct ModeledInputValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModeledInputValidation<Op> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation + 'static> ErasedOperation for ModeledInputValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(refused) = refuse_invalid_input::<Op>(svc, req) {
                return refused;
            }
            // Valid: the generated operation serves it, through the framework's own
            // deserialize -> validate -> handle -> serialize sequence (which finds
            // nothing left to refuse).
            match modeled_operation(svc, req) {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable: this gate is only reached through `override_operation`,
                // which resolved its key from that same routing table.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}

/// The operation the model's routing table names for this request.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// `Some(response)` when `Op`'s input violates a constraint the model declares: this
/// service's modelled `ValidationException`, 400, with both of its members set.
///
/// `None` when the request satisfies the model, or when it failed to READ for a reason
/// that is not a constraint violation — an unparseable body is a protocol failure, and
/// the framework words those per protocol.
fn refuse_invalid_input<Op: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    let protocol = svc.definition().protocol();
    let violation = first_violation::<Op>(protocol, req)?;
    let error = crate::state::member_validation_failed(&violation);
    protocol
        .serialize_error(
            &error,
            &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_CODE,
            VALIDATION_EXCEPTION_STATUS,
        )
        // A modelled error that will not serialize leaves the request to the framework,
        // which refuses it in the generic envelope — the very divergence this gate
        // removes, so it is logged rather than passed over silently.
        .inspect_err(|error| {
            tracing::warn!(
                operation = Op::NAME,
                %error,
                "the modelled ValidationException would not serialize; this refusal falls \
                 back to the framework's generic envelope"
            );
        })
        .ok()
}

/// The first constraint `Op`'s input violates, as the framework's own walk reports it.
///
/// Both validation classes are classified by the framework's own
/// [`smithy_server_runtime::refusal_of`], so this carries no second copy of "what counts
/// as a constraint refusal": a value outside a declared bound is found by walking what
/// was read, and a required member absent from the request is found while READING it, and
/// `refusal_of` already states that they are one refusal. Anything it does not recognise
/// (an unreadable body, a missing header) is not a constraint refusal and is left to the
/// framework, which words it per protocol.
///
/// AWS names ONE member per `ValidationException`, and the walk visits members in
/// schema order, so "the first" is a property of the input shape rather than of the
/// request.
fn first_violation<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<Violation> {
    let error = match protocol.deserialize_input(req, Op::INPUT_SCHEMA) {
        Err(unreadable) => unreadable,
        Ok(mut deserializer) => {
            match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
                Err(absent_or_malformed) => ProtocolError::Serde(absent_or_malformed),
                // The request was read: the model's own constraint traits decide. This
                // is the same call the centralized sequence makes, so no rule is
                // restated here.
                Ok(input) => {
                    let violations = validate(&input, Op::INPUT_SCHEMA);
                    if violations.is_empty() {
                        return None;
                    }
                    ProtocolError::Invalid(violations)
                }
            }
        }
    };
    match smithy_server_runtime::refusal_of(&error)? {
        Refusal::Invalid(violations) => violations.into_iter().next(),
        // Authentication is settled by the router before any operation runs, so an
        // `Unauthenticated` refusal cannot arrive here — and it is not a constraint
        // violation in any case.
        _ => None,
    }
}

/// The protocol this service answers modelled requests in: the model's own, with the one
/// framing correction the module docs state.
///
/// Implemented on the service itself because the model-derived protocol it delegates to is
/// owned by the generated entry and reached by reference (`self.entry.protocol()`), and
/// [`Service::override_protocol`] hands back a borrow of `&self` — so there is one protocol
/// instance, the generated one, and no second routing table to drift from the model.
impl<Eng: Send + Sync + 'static> ServerProtocol for EmulatedService<Eng> {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &aws_smithy_schema::ShapeId {
        self.entry.protocol().protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        self.entry.protocol().routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn aws_smithy_schema::serde::ShapeDeserializer + 'a>, ProtocolError> {
        self.entry.protocol().deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn aws_smithy_schema::serde::SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let response = self.entry.protocol().serialize_output(
            output,
            output_schema,
            operation_name,
            success_status,
        )?;
        // The status, the body and every other header are the protocol's: only the framing
        // of an answer that carries no content is corrected, by the ONE reading of that rule
        // every service in this tree shares.
        Ok(::service_common::no_content_responses::without_content(
            response,
        ))
    }

    fn serialize_error(
        &self,
        error: &dyn aws_smithy_schema::serde::SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        let response = self
            .entry
            .protocol()
            .serialize_error(error, error_schema, error_code, http_status)?;
        // Every modelled failure of this service carries a message and no failure of it is
        // modelled at the no-content status, so this is the same pass-through the shared
        // rule makes for any answer that carries content; it goes through the same function
        // rather than leaving one of the two paths to answer differently.
        Ok(::service_common::no_content_responses::without_content(
            response,
        ))
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn aws_smithy_schema::serde::SerializableStruct,
        event_schema: &Schema,
    ) -> Result<RtResponse, ProtocolError> {
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
        // A request whose resolved route key is this service's signing name is the
        // control plane, exactly as the framework's default claims it.
        if smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // And a request addressed to one of the data-plane ADDRESSES this service
        // PUBLISHES — one of AWS's own per-group hostnames, whose opaque first label no
        // route key can name, or a path on this emulator's own endpoint under the
        // reserved data-plane namespace. See the module docs: without this the router
        // answers `404 UnknownService` for a URL this service handed out.
        crate::state::addressed_data_plane(request_host(parts), parts.uri.path()).is_some()
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // A request to a published data-plane URL that PRESENTS a SigV4 credential is named
        // as an operation, so the router verifies that credential's signature before
        // anything is served — the router authenticates only what it names an operation
        // for. A data-plane request carrying NO credential is left unnamed on purpose: it
        // is answered by `override_routing_error` below, which the router reaches before
        // authentication, so an anonymous player and an unauthenticated encoder still get
        // this service's own answer rather than the emulator's "sign this request".
        if carries_sigv4_credential(req) && data_plane_of_request(req).is_some() {
            return Some(&self.credentialed_data_plane);
        }
        // Put this service's input-validation gate in front of the modelled operation
        // the request names, so a value outside a constraint the model declares is
        // refused with the MODELLED `ValidationException` (both members set) instead of
        // the framework's generic 400 envelope. The gate then hands every request the
        // model accepts to the generated operation, which the routing table still owns;
        // a request that names no modelled operation — a data-plane URL, say — resolves as
        // it always did, which is what leaves it to `override_routing_error`.
        let key = self.entry.protocol().routing_key(req).ok()?;
        let gate = self.input_validation.get(key)?;
        Some(gate.as_ref())
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by
        // the protocol returned here. See the module docs: it is the model's own protocol,
        // with one correction — an answer with no body is framed with `content-length: 0`
        // instead of with no framing header at all.
        Some(self)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // mediapackagev2 does NOT word a refused request the way the framework does.
        // Every input failure this service reports — whether the request boundary found
        // it from the model's own `@range`/`@length`/`@pattern`/enum constraints, or a
        // handler found a cross-member rule the model cannot express — is a 400
        // `ValidationException` whose message reads
        // `Validation error encountered at request.<member>: <the violated rule>`.
        // Leaving the framework's `N validation errors detected: Value '..' at '..'
        // failed to satisfy constraint: ..` sentence here would give one service two
        // unrelated validation-text families, so a client matching this service's
        // documented wording would not recognise half of its rejections.
        //
        // The code stays `ValidationException` (what the framework already chose and
        // what the model declares), so code and message move together.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "ValidationException".to_string(),
                message: crate::state::refusal_message(violations),
            }),
            // Authentication is the emulator's, not this service's: leave its wording.
            Refusal::Unauthenticated => None,
            // `Refusal` is `#[non_exhaustive]`: a refusal class added later keeps the
            // framework's wording until this service has a form for it.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation. A data-plane request that carries no SigV4 credential is
        // exactly that case by construction (`override_operation` names an operation
        // only for the credentialed ones), so this is where an anonymous fetch of a
        // published egress URL and an unauthenticated push to a published ingest URL are
        // served. Anything else keeps the framework's protocol-correct default.
        match data_plane_of_request(req) {
            Some(addressed) => match addressed.plane {
                crate::state::DataPlane::Egress => {
                    Some(serve_egress(&*self.engine, req, &addressed))
                }
                crate::state::DataPlane::Ingest => {
                    Some(serve_ingest(&*self.engine, req, &addressed))
                }
            },
            None => None,
        }
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

/// Serve one request to a published EGRESS URL.
///
/// The origin endpoint's resource policy decides, against the action the request's
/// method needs ([`egress_action`]): without a grant to the `*` principal the fetch is
/// refused `403`, and the refusal is the same whether the endpoint is absent or merely
/// ungranted, so a caller cannot probe for endpoints it may not read. With the grant
/// the fetch is authorized and the object is resolved, which is `404` while nothing
/// has been packaged for it.
///
/// Free-standing, and reached with the ONE shared engine, so the two seams that serve the
/// data plane — [`Service::override_routing_error`] for an anonymous request and
/// [`CredentialedDataPlane`] for a credentialed one — serve it identically. Identically
/// includes the grant that is read: the decision is always the one made for the `*`
/// principal, because this emulator models no caller IDENTITY to match a statement's
/// `Principal` against (see the module docs) — a credentialed fetch is verified, then
/// authorized by the same `*` grant an anonymous one is. A statement naming one principal
/// is therefore not a grant this emulator can honour, for either seam.
fn serve_egress<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    addressed: &crate::state::AddressedDataPlane,
) -> RtResponse {
    if !matches!(req.method(), "GET" | "HEAD") {
        return error_response(
            405,
            "MethodNotAllowed",
            "The egress data plane serves GET and HEAD",
        );
    }
    let Some(target) = crate::state::parse_egress_path(&addressed.path) else {
        return error_response(
            404,
            "NotFound",
            "The requested path is not an egress path this service publishes",
        );
    };
    let Some(state) = crate::state::service_state(engine) else {
        return error_response(
            500,
            "InternalServerException",
            "the MediaPackageV2 store is unavailable",
        );
    };
    let region = addressed.region.as_str();
    let store = state.lock();
    let granted = store.endpoint_at(region, &target).is_some_and(|endpoint| {
        endpoint.policy.as_ref().is_some_and(|attached| {
            crate::state::policy_allows_anonymous(
                &attached.policy,
                egress_action(req.method()),
                // The resource the request addresses, which a statement's `Resource`
                // is matched against: the endpoint the published URL names.
                &endpoint.arn,
            )
        })
    });
    if !granted {
        return error_response(403, "AccessDenied", ACCESS_DENIED_MESSAGE);
    }
    // Authorized, and there is nothing to serve: this emulator runs no packager, so
    // no egress object exists for an endpoint however much has been ingested. The
    // answer is the honest one for a data plane — no such object — not a synthesised
    // manifest.
    error_response(
        404,
        "NotFound",
        "No content is available at this origin endpoint yet",
    )
}

/// Serve one request to a published INGEST URL.
///
/// MediaPackage returns no credential for ingest, so an unauthenticated push is
/// refused `403` here — it carries no signature for the emulator to judge, and the router
/// hands this seam the request before it authenticates anything. A push that DOES carry a
/// SigV4 credential reaches this function only through [`CredentialedDataPlane`], i.e.
/// only once the emulator's authenticator accepted that credential, and is recorded
/// against the channel, bounded by [`crate::state::MAX_INGESTED_OBJECTS_PER_CHANNEL`] and
/// [`crate::state::MAX_INGESTED_OBJECT_BYTES`]. A path under a channel that publishes no
/// ingest endpoint is not a path this service published, so it resolves to nothing (`404`)
/// however the push is credentialed.
fn serve_ingest<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    addressed: &crate::state::AddressedDataPlane,
) -> RtResponse {
    if !matches!(req.method(), "PUT" | "POST") {
        return error_response(
            405,
            "MethodNotAllowed",
            "The ingest data plane serves PUT and POST",
        );
    }
    if !carries_sigv4_credential(req) {
        return error_response(403, "AccessDenied", ACCESS_DENIED_MESSAGE);
    }
    let Some(target) = crate::state::parse_ingest_path(&addressed.path) else {
        return error_response(
            404,
            "NotFound",
            "The requested path is not an ingest path this service publishes",
        );
    };
    let Some(state) = crate::state::service_state(engine) else {
        return error_response(
            500,
            "InternalServerException",
            "the MediaPackageV2 store is unavailable",
        );
    };
    let body = req.body().bytes().unwrap_or_default();
    if body.len() > crate::state::MAX_INGESTED_OBJECT_BYTES {
        return error_response(
            413,
            "RequestEntityTooLarge",
            "The ingested object is larger than this emulator retains",
        );
    }
    let region = addressed.region.as_str();
    let mut store = state.lock();
    // Both store bounds are judged under this one hold of the lock, so two concurrent
    // pushes cannot both see room that only one of them has.
    match store.record_ingested_object(region, &target, body.to_vec()) {
        crate::state::IngestOutcome::Recorded => {}
        crate::state::IngestOutcome::NoSuchChannel => {
            return error_response(
                404,
                "NotFound",
                "The requested path is not an ingest path this service publishes",
            );
        }
        crate::state::IngestOutcome::ObjectCountReached => {
            return error_response(
                507,
                "InsufficientStorage",
                "This channel already holds as many ingested objects as this emulator \
                 retains",
            );
        }
        crate::state::IngestOutcome::StoreFull => {
            return error_response(
                507,
                "InsufficientStorage",
                "This emulator is already retaining as many ingested bytes as it holds",
            );
        }
    }
    let mut resp = RtResponse::new(
        StatusCode::try_from(200u16).expect("200 is a valid status"),
        SdkBody::empty(),
    );
    resp.headers_mut().insert("content-length", "0");
    resp
}

/// What a request addresses on this service's data plane, or `None` when it addresses
/// none of it.
///
/// Both addressings are read by the ONE reader of them
/// ([`crate::state::addressed_data_plane`]): the published HOSTNAME, which a client that
/// resolves AWS's own hosts sends, and a PATH on this emulator's own endpoint, which is
/// what a client on this machine can reach. The `Host` header is what a buffered HTTP/1
/// request carries; an HTTP/2 request carries the same authority on its URI instead.
fn data_plane_of_request(req: &RtRequest) -> Option<crate::state::AddressedDataPlane> {
    crate::state::addressed_data_plane(data_plane_host(req), request_path(req.uri()))
}

/// The `Host` header of a runtime request, or the authority of its URI when it carries
/// none (an HTTP/2 request carries `:authority` on the URI instead).
fn data_plane_host(req: &RtRequest) -> Option<&str> {
    req.headers().get("host").or_else(|| authority_of(req.uri()))
}

/// The action an anonymous player needs on an origin endpoint to fetch its content with
/// `GET`. It is the action MediaPackage's own endpoint-policy examples grant.
const EGRESS_GET_ACTION: &str = "mediapackagev2:GetObject";

/// The action a `HEAD` of the same object needs, which those examples grant separately:
/// a policy may allow the metadata read without allowing the body.
const EGRESS_HEAD_ACTION: &str = "mediapackagev2:GetHeadObject";

/// The action an egress request of this method is authorized against.
fn egress_action(method: &str) -> &'static str {
    match method {
        "HEAD" => EGRESS_HEAD_ACTION,
        _ => EGRESS_GET_ACTION,
    }
}

/// Whether the request presents a SigV4 credential at all — in the `Authorization`
/// header a signed SDK request carries, or in the `X-Amz-Credential` query parameter a
/// presigned URL carries.
///
/// Only PRESENCE is read. Whether the signature over that credential is correct is the
/// emulator's own check, never a service's.
fn carries_sigv4_credential(req: &RtRequest) -> bool {
    if req
        .headers()
        .get("authorization")
        .is_some_and(|value| value.contains("Credential="))
    {
        return true;
    }
    request_query(req.uri())
        .split('&')
        .any(|pair| pair.starts_with("X-Amz-Credential="))
}

/// The `Host` header of a buffered request, or the URI authority when the request is
/// HTTP/2 (which carries `:authority` on the URI instead of a `Host` header).
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => Some(host),
        None => parts.uri.authority().map(http::uri::Authority::as_str),
    }
}

/// The authority of an absolute request URI, or `None` when the URI is origin-form.
fn authority_of(uri: &str) -> Option<&str> {
    let (_scheme, rest) = uri.split_once("://")?;
    Some(match rest.find('/') {
        Some(path_start) => &rest[..path_start],
        None => rest,
    })
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn request_path(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan over
    // the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

/// The query component of a request URI: everything after the first `?`.
fn request_query(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}

/// One data-plane error response: the status, the code in `x-amzn-errortype`, and a JSON
/// body carrying the reason.
///
/// The data plane is plain HTTPS rather than a modelled operation, so it answers in its
/// own small envelope instead of the service's restJson1 one. The message is built from
/// constants, never from the request.
fn error_response(status: u16, code: &str, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "message": message, "code": code }).to_string();
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", JSON_CONTENT_TYPE);
    resp.headers_mut()
        .insert("x-amzn-errortype", code.to_string());
    resp
}

#[cfg(test)]
mod tests {
    /// Every operation the model declares has an input-validation gate.
    ///
    /// The gate set is written out by name in `input_validation_gates` — the operation
    /// TYPE is what names the input shape and its schema, and the erased routing table
    /// cannot hand that back — so this is what keeps the two in step: an operation added
    /// to the model (and so to `src/ops/`) without a gate would quietly go back to
    /// answering a constraint refusal in the framework's generic envelope, and this test
    /// fails naming it.
    #[test]
    fn every_modeled_operation_has_an_input_validation_gate() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json"
        );
        let model = std::fs::read_to_string(path).expect("this crate ships its Smithy model");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the Smithy model is JSON");
        let shapes = model
            .get("shapes")
            .and_then(serde_json::Value::as_object)
            .expect("a Smithy model has shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape.get("type").is_some_and(|kind| kind == "operation"))
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            !operations.is_empty(),
            "the model declares no operations, so this test proves nothing"
        );
        let gates = super::input_validation_gates::<engine::InMemoryEngine>();
        for operation in &operations {
            assert!(
                gates.contains_key(*operation),
                "operation {operation} has no input-validation gate, so a request that \
                 violates its model would be refused without the modelled members"
            );
        }
        assert_eq!(
            gates.len(),
            operations.len(),
            "the gate set and the model's operations must match exactly, got {} gates for \
             {} operations",
            gates.len(),
            operations.len()
        );
    }
}
