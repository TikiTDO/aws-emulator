// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-appconfigdata-appconfigdata` service: how it claims requests and how it words the
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
//!   correctly. This service uses it: each of its two routes is pre-checked against
//!   that operation's own input schema, so an input refusal is answered in the
//!   operation's modelled error shape and a poll carrying no `configuration_token`
//!   reaches the operation at all rather than matching no route.
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

use aws_smithy_schema_server::{constraints::path, ProtocolError, Violation, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused,
    ServerHttpProtocol, Service,
};
use std::marker::PhantomData;
use std::sync::Arc;

/// One modelled route of this service: its `@http` method and URI, and the operation
/// the generated routing table wires under it.
struct ModeledRoute {
    method: &'static str,
    path: &'static str,
    operation: &'static str,
}

/// This service's whole routed surface: both rows are the `@http` method and URI each
/// operation declares in this crate's `smithy-model/` document, and are listed in the
/// crate root's `OPERATIONS.md` ("Route: POST /configurationsessions -> 201" and
/// "Route: GET /configuration -> 200").
///
/// It is read only to name the operation a request is addressed to when the model's own
/// routing key cannot — see [`ModeledInputCheck`]. Which handler serves the request is
/// still the generated routing table's answer; this table holds no behaviour.
const MODELED_ROUTES: &[ModeledRoute] = &[
    ModeledRoute {
        method: "POST",
        path: "/configurationsessions",
        operation: "StartConfigurationSession",
    },
    ModeledRoute {
        method: "GET",
        path: "/configuration",
        operation: "GetLatestConfiguration",
    },
];

/// The modelled route `req` is addressed to, by method and URI path, or `None` for a
/// request this service does not route.
fn matching_route(req: &aws_smithy_runtime_api::http::Request) -> Option<&'static ModeledRoute> {
    let uri = req.uri().parse::<http::Uri>().ok()?;
    let path = uri.path().trim_matches('/').to_string();
    MODELED_ROUTES
        .iter()
        .find(|route| route.method == req.method() && route.path.trim_matches('/') == path)
}

/// The validation path of `GetLatestConfiguration`'s one required member, as the
/// constraint vocabulary spells it.
fn configuration_token_path() -> String {
    path::member("ConfigurationToken")
}

/// How this service states the refusal `violations` describe, or `None` when they
/// describe none.
///
/// Every violation but one earns the standard request-derived validation envelope
/// naming the member and the constraint it failed — built by the framework's own
/// renderer, so an absent member and a value outside a modelled bound read one way. An
/// absent `ConfigurationToken` earns the service's own sentence, the same one the
/// operation answers a blank value with; see
/// [`crate::ops::get_latest_configuration::CONFIGURATION_TOKEN_REQUIRED_MESSAGE`].
fn refusal_message(violations: &[Violation]) -> Option<String> {
    let absent_token = violations.iter().any(|violation| {
        violation.kind == ViolationKind::MissingRequiredMember
            && violation.path == configuration_token_path()
    });
    if absent_token {
        return Some(
            crate::ops::get_latest_configuration::CONFIGURATION_TOKEN_REQUIRED_MESSAGE.to_string(),
        );
    }
    aws_smithy_schema_server::envelope(violations)
}

/// The wire protocol a refusal this file serializes is framed in: the service's own
/// [`Service::override_protocol`] when it takes that seam, else the model-derived one —
/// the same selection the framework's centralized walk makes, so a refusal serialized
/// here cannot be framed in a different codec from the responses that walk serializes
/// beside it.
fn effective_protocol(svc: &dyn Service) -> &dyn ServerHttpProtocol {
    svc.override_protocol()
        .unwrap_or_else(|| svc.definition().protocol())
}

/// Claims one operation's route so the request reaches that operation at all, and words
/// every refusal of its input in the operation's modelled error shape — then hands the
/// framework's own answer back untouched.
///
/// Both operations model `BadRequestException` (HTTP 400) as the answer to input that
/// "fails to satisfy the constraints specified by the service", which is exactly what
/// an absent `@required` member or a value outside a modelled bound is. Two things make
/// the framework answer such a request differently, and both are why this sits here:
///
/// * the framework's refusal envelope states the message under its own key, not under the
///   `Message` member this service's `BadRequestException` declares, so a client reading
///   the modelled shape finds no message at all; and
/// * a route whose `@required` query binding the request omits matches no operation at
///   all — `GetLatestConfiguration` binds `ConfigurationToken` to `@httpQuery` — so a
///   tokenless poll would be answered `501 NotImplemented`, a server-fault class an SDK
///   does not treat as the client error it is.
///
/// Nothing about WHICH input is wrong is decided here: the violations come from
/// `service_common::validation::refusing_violations`, the shared collecting half built
/// on the framework's own machinery (the protocol's deserializer plus
/// `constraints::validate` over the built input), so this wrapper cannot find a
/// different violation set from the walk that refused the request, and a model change
/// needs no change here. A failure that is not a refusal of the input — a body the codec
/// cannot read at all — yields no violations, so the framework's answer stands.
///
/// That collecting walk runs over a request the framework REFUSED, never over one it
/// served, so a served request carries the walk's cost once rather than twice. For a
/// POLLING data plane the difference is not incidental: the `Token` shape's `@pattern` is
/// the dearest check in the walk, every poll carries a token, and a client watching a
/// deployment roll out makes one poll per session it samples.
struct ModeledInputCheck<O> {
    /// The operation whose schema is checked; its `INPUT_SCHEMA`, `Input` type and
    /// `NAME` are all that is used, so no instance of it is held — the generated
    /// routing table owns the one instance built from the shared engine.
    operation: PhantomData<fn() -> O>,
}

impl<O> ModeledInputCheck<O> {
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<O: Operation> ErasedOperation for ModeledInputCheck<O> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The framework's own walk decides the request: deserialize, validate,
            // handle, serialize, plus its telemetry over the same bytes. Reached through
            // the routing table rather than through `override_operation`, which would
            // route straight back here.
            let answered = match svc.definition().routing_table().get(O::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                None => {
                    return svc.definition().framework_error_response(
                        ProtocolError::UnknownOperation(O::NAME.to_string()),
                        req,
                    );
                }
            };
            // A request the framework SERVED is answered as it answered it. Only a
            // refusal is looked at, and then only to state it in the modelled error
            // shape.
            if answered.status().is_success() {
                return answered;
            }
            // The protocol is read, used and dropped inside this one block:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across an await
            // would make this future non-`Send`.
            let reworded = {
                let protocol = effective_protocol(svc);
                let violations =
                    service_common::validation::refusing_violations::<O>(protocol, req);
                refusal_message(&violations).map(|message| {
                    let error = crate::ops::types::BadRequestException {
                        message: Some(message),
                        // `Reason` and `Details` describe a parameter the service could
                        // not USE — the modelled `InvalidParameterProblem` values are
                        // `Corrupted`, `Expired` and `PollIntervalNotSatisfied` — and a
                        // member that is absent or out of bounds is none of those, so
                        // neither is reported.
                        reason: None,
                        details: None,
                    };
                    protocol
                        .serialize_error(
                            &error,
                            &crate::ops::types::SCHEMA_BAD_REQUEST_EXCEPTION,
                            BAD_REQUEST_EXCEPTION,
                            BAD_REQUEST_EXCEPTION_STATUS,
                        )
                        .unwrap_or_else(|error| {
                            svc.definition().framework_error_response(error, req)
                        })
                })
            };
            match reworded {
                Some(response) => {
                    tracing::debug!(
                        operation = O::NAME,
                        outcome = BAD_REQUEST_EXCEPTION,
                        "refused an input the model does not allow"
                    );
                    response
                }
                None => answered,
            }
        })
    }
}

/// The modelled error code and status every refusal above answers with.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";
/// See [`BAD_REQUEST_EXCEPTION`]: the shape's `@httpError`.
const BAD_REQUEST_EXCEPTION_STATUS: u16 = 400;

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of this rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment
/// must be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact
/// segment equality. That path conjunct is what separates two services sharing one
/// SigV4 signing name — AppConfigData and the AppConfig control plane both sign as
/// `appconfig`, so the key alone matches both and the router would reject every such
/// request as `500 AmbiguousService` — and their modelled path spaces are disjoint
/// (`configuration`, `configurationsessions` here; `applications`,
/// `deploymentstrategies`, `extensions`, … there).
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

/// The `com-amazonaws-appconfigdata-appconfigdata` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Checks `StartConfigurationSession`'s input against its own schema before the
    /// handler runs; see [`ModeledInputCheck`]. Held on the service because
    /// [`Service::override_operation`] hands back a borrow of `&self`.
    start_configuration_session_check:
        ModeledInputCheck<crate::ops::start_configuration_session::StartConfigurationSession<Eng>>,
    /// The same check for `GetLatestConfiguration`.
    get_latest_configuration_check:
        ModeledInputCheck<crate::ops::get_latest_configuration::GetLatestConfiguration<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            start_configuration_session_check: ModeledInputCheck::new(),
            get_latest_configuration_check: ModeledInputCheck::new(),
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
        // Delegated to the framework's own predicate — see [`can_handle_parts`]:
        // this service's signing name (`appconfig`) is shared with the AppConfig
        // control plane, so the claim is the route key AND the model-derived first
        // URI path segment. Matching on the signing name alone would have both
        // services claim every `appconfig`-signed request, which the router answers
        // with 500 `AmbiguousService` instead of serving either one.
        can_handle_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every request to one of this service's two routes is checked against that
        // operation's own input schema first, so a refusal of the input is answered in
        // the operation's modelled error shape rather than in the framework's
        // pre-dispatch envelope — and so a poll that carries no `configuration_token`
        // reaches the operation at all instead of matching no route. The check hands a
        // well-formed request straight to the generated handler. See
        // [`ModeledInputCheck`].
        match matching_route(req)?.operation {
            "StartConfigurationSession" => Some(&self.start_configuration_session_check),
            "GetLatestConfiguration" => Some(&self.get_latest_configuration_check),
            _ => None,
        }
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Both operations model `BadRequestException` (HTTP 400) as the answer to
        // input that "fails to satisfy the constraints specified by the service",
        // and that is exactly what the framework refused here — an absent required
        // identifier, an identifier outside 1..128, or a
        // `RequiredMinimumPollIntervalInSeconds` outside 15..86400. So this service
        // names its own modelled error code rather than the framework's generic
        // `ValidationException`; the status (400) belongs to the protocol and is the
        // same either way. The message keeps the framework's validation envelope,
        // which names the member and the constraint it failed.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "BadRequestException".to_string(),
                message: aws_smithy_schema_server::envelope(violations).unwrap_or_else(|| {
                    "The input fails to satisfy the constraints specified by the service."
                        .to_string()
                }),
            }),
            // An authentication refusal is not a modelled input error: leave the
            // framework's 403 wording alone.
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
