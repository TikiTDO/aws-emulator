// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE: every request is served through the
//!   authorization gate, which decides whether IAM permits the action the request named
//!   (and, for an operation that acts on a caller-named EC2 instance, whether the account
//!   owns that instance) BEFORE the framework deserializes or constraint-checks anything,
//!   and then hands an authorized request to the very operation the model's own routing key
//!   names. A key the model does not name still resolves to `None` here, so the router's
//!   routing-error path is unchanged.
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
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service, ViolationKind,
};
use std::sync::Arc;

/// Whether IAM lets a request perform the action it names — the decision taken ahead of
/// EVERY operation of this service.
mod authorization;

/// The envelope a refusal of this service leaves under: its status, the ONE error-type
/// header that names its shape, and its `message` body.
mod refusal_headers;

use refusal_headers::refusal_response;

/// The HTTP method every gated route below is bound to: each one is
/// `@http(method: "POST", …)`, so a request arriving at one of those paths under any other
/// method names no gated operation and the gate does not apply to it.
const GATED_METHOD: &str = "POST";

/// One operation whose instance-authorization refusal is decided BEFORE the framework
/// examines the request's shape.
struct InstanceGatedRoute {
    /// The path the operation's `@http` `uri` binds it to, matched before the framework has
    /// resolved a routing key.
    uri: &'static str,
    /// The operation the model names for that route. It is both the routing key the
    /// modelled operation is wired under — so an authorized request is served by running
    /// it — and the operation name a gated request is recorded under, so a refusal lands in
    /// the same telemetry series as the requests that reach the handler.
    operation: &'static str,
    /// The member of this operation's input that addresses one of its own resources by ARN,
    /// when the model declares one, as a client spells it on the wire.
    ///
    /// An operation whose input carries one can name the instance it acts on THAT way
    /// instead of by `InstanceId` — the ARN identifies a user association on an instance —
    /// so the gate resolves it for a request that names no instance of its own. Only
    /// `DisassociateUser` declares such a member (`smithy_query(query="operation",
    /// operation="AssociateUser,ListUserAssociations,DisassociateUser")`: `InstanceUserArn`,
    /// and it is the only one of the three whose `InstanceId` is not required), so the other
    /// two routes carry `None`.
    arn_member: Option<&'static str>,
}

/// Every route the authorization gate covers.
///
/// An operation belongs here exactly when its input carries an `InstanceId`: the account's
/// ownership of the EC2 instance a request names is what decides whether this service acts
/// on that request at all, so the decision is taken for every operation that names one.
/// That is `AssociateUser`, `ListUserAssociations` and `DisassociateUser`, the three whose
/// input declares the member (`smithy_query(query="operation",
/// operation="AssociateUser,ListUserAssociations,DisassociateUser")`), and whose handlers
/// each open with that same gate ([`crate::state::ec2_fleet::authorize_instance`]). Listing
/// them in ONE table is what keeps the hoisted gate
/// from covering some of them and not others: the refusal a caller gets for an instance
/// this service will not authorize cannot depend on which of those operations it called,
/// nor on what else the request got wrong.
///
/// Whether the model marks the member `@required` changes only WHEN the gate applies, not
/// whether it does: `DisassociateUser` marks nothing required and can address an
/// association by `InstanceUserArn` alone, so a request of it that names no instance is
/// gated on the association that ARN names instead ([`InstanceGatedRoute::arn_member`]);
/// one that names neither has nothing to authorize and goes on to the modelled operation.
static INSTANCE_GATED_ROUTES: &[InstanceGatedRoute] = &[
    InstanceGatedRoute {
        uri: "/user/AssociateUser",
        operation: "AssociateUser",
        arn_member: None,
    },
    InstanceGatedRoute {
        uri: "/user/ListUserAssociations",
        operation: "ListUserAssociations",
        arn_member: None,
    },
    InstanceGatedRoute {
        uri: "/user/DisassociateUser",
        operation: "DisassociateUser",
        arn_member: Some("InstanceUserArn"),
    },
];

/// The member every tagging operation addresses its resource by, as the violation path the
/// boundary validator reports for it.
///
/// `ListTagsForResource`, `TagResource` and `UntagResource` each declare `ResourceArn`
/// with the service's `@pattern` for an ARN; a value outside it is this service's own
/// `BadRequestException`, not the generic `ValidationException` the validator would name.
/// A violation path spells a member in lowerCamel (`aws_smithy_schema_server::constraints`
/// `path::member`), which is the spelling matched here.
const RESOURCE_ARN_MEMBER: &str = "resourceArn";

/// The `com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// THE GATE every request this service serves is run through: whether IAM permits the
    /// action the request names, and — for a request that names one of this service's own
    /// resources on an EC2 instance — whether the account owns that instance.
    ///
    /// Both decisions have to be taken BEFORE the framework deserializes and
    /// constraint-checks the request, which is the one thing that cannot be done from
    /// inside a handler, and the instance half means resolving an EC2 instance against the
    /// service that owns instances, which is an await. So the gate is an operation: the
    /// sync [`Service::override_operation`] seam only picks it, and the gate itself decides
    /// and then either refuses or runs the modelled operation the request named.
    gate: AuthorizationGate<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            gate: AuthorizationGate {
                engine: Arc::clone(&engine),
                instances: INSTANCE_GATED_ROUTES
                    .iter()
                    .map(|route| InstanceGate {
                        route,
                        engine: Arc::clone(&engine),
                    })
                    .collect(),
            },
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
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // THE AUTHORIZATION GATE, and the only seam it can run in.
        //
        // A request is authorized before its shape is validated, on both of the facts that
        // decide whether this service acts on it at all: whether the IAM policies in force
        // on the principal its credentials speak for allow the action it named
        // ([`authorization::authorize`]), and — for an operation that acts on a
        // caller-named EC2 instance — whether this account owns that instance. The same
        // call a gate refuses is refused identically whatever else the request gets wrong —
        // whether its `Tags` map is within the modelled `@length` or past it, and whether or
        // not it even carries the members the model requires. The framework's centralized
        // walk is deserialize -> validate -> handle, so a handler runs only AFTER
        // modelled-constraint enforcement; this hook is consulted before any of it, which is
        // why the decisions live here and not in the handlers.
        //
        // NOT a re-route: the gate serves the very operation the model's own routing key
        // names, so a key the model does not name resolves to `None` here exactly as it
        // does in the generated table and the router's routing-error path is unchanged.
        // The IAM half covers EVERY modelled operation, and the instance half every
        // operation that names an instance ([`INSTANCE_GATED_ROUTES`]), so neither refusal
        // depends on which operation the caller reached for; each handler that resolves an
        // instance runs that same gate again
        // ([`crate::state::ec2_fleet::authorize_instance`]), so the two cannot disagree
        // about a request that gets this far.
        //
        // What is picked here is the GATE, not the refusal: resolving the instance reaches
        // the service that owns instances, which cannot be awaited in this sync seam, so the
        // gate makes its decisions in its own `run` and hands an authorized request on to
        // the modelled operation the request named.
        self.entry.protocol().routing_key(req).ok()?;
        Some(&self.gate as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // A `ResourceArn` outside this service's ARN `@pattern` is refused by the boundary
        // validator, which names every such refusal `ValidationException`. This service
        // names it `BadRequestException` instead — the code its tagging operations really
        // send for an ARN that is not usable, and the code a tag-management client branches
        // on. The handlers answer that same code for an ARN that is well formed and names
        // no resource (`crate::scaffolded::Scaffolded::invalid_input_resource_arn`), so one
        // refusal does not arrive under two codes depending on which layer caught it; only
        // the sentence differs, because only one of the two is about syntax.
        //
        // Only a PATTERN violation of that member is reworded: the sentence says the ARN's
        // syntax is wrong, which is true of nothing else. It is reworded whenever the set
        // CONTAINS one, not only when it is the whole set — an ARN this service cannot use
        // is answered the same way whatever else the request also got wrong, which is the
        // rule a tag-management client branches on. A refusal with no such violation keeps
        // the framework's wording, which is what returning `None` leaves in place.
        let Refusal::Invalid(violations) = refusal else {
            return None;
        };
        let bad_arn_syntax = |violation: &smithy_server_runtime::Violation| {
            violation.path == RESOURCE_ARN_MEMBER
                && matches!(violation.kind, ViolationKind::Pattern(_))
        };
        if violations.iter().any(bad_arn_syntax) {
            return Some(Refused {
                code: crate::scaffolded::BAD_REQUEST_EXCEPTION_CODE.to_string(),
                message: crate::scaffolded::INVALID_INPUT_RESOURCE_ARN_SYNTAX_MESSAGE.to_string(),
            });
        }
        // Every other constraint refusal keeps the framework's envelope and its clauses —
        // with ONE rendering this service owns: a `@pattern` requirement quotes the
        // expression WITHOUT the `^` / `$` anchors the model writes around it. The clause
        // already says the MEMBER must satisfy the expression, so the anchors that make the
        // model's find total are the model's way of saying that and not part of the
        // character grammar a caller has to satisfy; a client that lifts the expression out
        // of the message and re-applies it locally must get the one the service advertises.
        //
        // A rendering rule, not a member rule: it is applied to whatever violation the
        // walk produced, so it holds for every `@pattern`-constrained member of every
        // operation of this service. A refusal carrying no pattern violation is left to the
        // framework's own wording, which is what returning `None` keeps.
        unanchored_pattern_refusal(violations)
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

/// The wire error code this service states an input-constraint refusal with: its own
/// modelled `ValidationException`, `@httpError 400`
/// (`smithy_query(query="shape",shape="ValidationException")`). It is also the code the
/// framework's own pre-dispatch renderer picks for such a refusal, so restating a clause
/// changes the MESSAGE and nothing else about the answer.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The constraint envelope for `violations`, with every `@pattern` requirement quoted
/// without the anchors the model writes around it — or `None` when no violation states a
/// `@pattern`, which leaves the framework's own wording in place.
///
/// Built by handing the framework its OWN clause renderer a violation whose expression is
/// already unanchored ([`aws_smithy_schema_server::constraints::envelope`]), so the
/// envelope, the member path, the echoed value and the requirement wording of every other
/// constraint class stay the framework's and cannot drift from the rest of the emulator:
/// the one thing this service states differently is the expression.
fn unanchored_pattern_refusal(violations: &[smithy_server_runtime::Violation]) -> Option<Refused> {
    if !violations
        .iter()
        .any(|violation| matches!(violation.kind, ViolationKind::Pattern(_)))
    {
        return None;
    }
    let stated: Vec<smithy_server_runtime::Violation> =
        violations.iter().map(unanchored_violation).collect();
    Some(Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message: aws_smithy_schema_server::constraints::envelope(&stated)?,
    })
}

/// `violation` with a `@pattern` expression stripped of the whole-value anchors the model
/// spells it with; every other violation is itself.
///
/// The strip is `service_common::validation::unanchored`'s — the one implementation of that
/// rule in this tree — rather than a second copy here.
fn unanchored_violation(
    violation: &smithy_server_runtime::Violation,
) -> smithy_server_runtime::Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => smithy_server_runtime::Violation {
            kind: ViolationKind::Pattern(service_common::validation::unanchored(pattern)),
            ..violation.clone()
        },
        _ => violation.clone(),
    }
}

/// THE GATE every request this service serves passes through, as an operation the dispatch
/// path can run: first whether IAM permits the ACTION the request named, then — for a
/// request that names one of this service's own resources on an EC2 instance — whether this
/// account owns that instance ([`InstanceGate`]).
///
/// The order is AWS's: authorization is decided by the IAM layer in FRONT of the service,
/// so it is answered before anything about the request's own resource is resolved, let
/// alone deserialized or constraint-checked. A request both gates pass is handed to the
/// modelled operation the request named, read off this service's own routing table, so it
/// is served by exactly the centralized deserialize -> validate -> handle walk every other
/// request takes: the gate changes WHICH refusal comes first, not whether the modelled
/// constraints are enforced.
///
/// A request it refuses answers one thing: the refusal the decision itself carries, rendered
/// through the service's own protocol envelope with the code and the status that refusal
/// names ([`refusal_response`]), so it is shaped exactly like every other refusal this
/// service sends.
struct AuthorizationGate<Eng> {
    /// The same shared engine the entry constructed each operation from, so the IAM
    /// decision reads the account's own IAM and STS stores and the instance half reads this
    /// service's own fleet seam.
    engine: Arc<Eng>,
    /// The instance-ownership gate for each route in [`INSTANCE_GATED_ROUTES`].
    instances: Vec<InstanceGate<Eng>>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for AuthorizationGate<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        let started = std::time::Instant::now();
        Box::pin(async move {
            // The operation the request named, read exactly as dispatch reads it: the
            // routing key this service's protocol resolves from the request line. It is the
            // ACTION the IAM decision is about, and the key the modelled operation is wired
            // under. Read and dropped inside this one statement, because
            // `&dyn ErasedServiceDefinition` is not `Sync` and the awaits below would make
            // this future non-`Send`; the key itself borrows the REQUEST.
            let Ok(operation) = svc.definition().protocol().routing_key(req) else {
                // `override_operation` picked this gate only for a request whose key
                // resolves, so this is unreachable; answered as the request naming no
                // modelled operation, which is what it would be.
                return svc.definition().routing_error_response(req);
            };
            // THE IAM DECISION, ahead of every other answer this service could give: a
            // caller whose permissions do not cover the action is told that and nothing
            // about the resource it named, and the call performs nothing.
            if let Err(refusal) = authorization::authorize(&*self.engine, ctx, operation) {
                let code = refusal.code;
                let response = refusal_response(svc, req, &refusal);
                record_refusal(
                    ctx,
                    operation,
                    code,
                    response.status().as_u16(),
                    started.elapsed(),
                );
                return response;
            }
            // The instance half, for the routes whose input names an EC2 instance.
            if req.method() == GATED_METHOD {
                // The path portion of the request URI, which is what an operation's `@http`
                // `uri` pattern is written against. Read through
                // `service_common::request_path`, the one shared reader for it in this tree,
                // rather than a local copy of the split: the framework's own is
                // `pub(crate)` in a read-only tree, and this crate depends on that crate
                // anyway.
                let path = ::service_common::request_path(req.uri());
                if let Some(gate) = self.instances.iter().find(|gate| gate.route.uri == path) {
                    return gate.run(svc, req, ctx).await;
                }
            }
            // The modelled operation the key names, from this service's own model-derived
            // routing table.
            if let Some(modelled) = svc.definition().routing_table().get(operation) {
                return modelled.run(svc, req, ctx).await;
            }
            svc.definition().routing_error_response(req)
        })
    }

    // `None`, NOT an empty list: the request is still signed as the operation the model
    // names. Answering `Some(&[])` here would serve every request of this service unsigned.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// The instance-ownership gate for one [`INSTANCE_GATED_ROUTES`] entry, as an operation the
/// dispatch path can run.
///
/// It is an operation rather than a predicate because the decision needs an await: the
/// account's EC2 instances are EC2's, so resolving the one a request names asks the service
/// that owns them over the in-process callback
/// ([`crate::state::ec2_fleet::instance_refusal`]), the same gate each handler asks.
/// Running here is what puts that decision AHEAD of the boundary validator.
///
/// An authorized request is handed to the modelled operation the route names, read off this
/// service's own routing table, so it is served by exactly the centralized
/// deserialize -> validate -> handle walk every other request takes — the gate changes WHICH
/// refusal comes first, not whether the modelled constraints are enforced. Its handler then
/// resolves the instance AGAIN, which is deliberate: each layer asks the question itself, so
/// the hoisted gate and the handler cannot disagree about a request, and a handler stays
/// correct for a caller that reaches it without this seam. The cost is one extra
/// `DescribeInstances` for one id, in process, on an authorized gated request.
///
/// A request it refuses answers one thing: the refusal the gate itself carries, which is the
/// very value a handler returns on its own error channel
/// ([`crate::state::ec2_fleet::authorize_instance`]) — so the code and the sentence are not
/// restated here and a caller cannot tell which gate refused the request, only that it was
/// refused. It is rendered through the service's own protocol envelope (`refusal_response`),
/// so a fail-closed refusal is shaped exactly like every other refusal this service sends.
///
/// The refusal travels through the service's own protocol envelope, with the STATUS the
/// refusal itself carries ([`refusal_response`]), so a fail-closed refusal answered here is
/// shaped exactly like the one a handler's error channel answers for the same fact.
struct InstanceGate<Eng> {
    /// The route this gate covers: the path it is picked for and the modelled operation it
    /// hands an authorized request to.
    route: &'static InstanceGatedRoute,
    /// The same shared engine the entry constructed each operation from, so the gate reads
    /// this service's own fleet seam before asking EC2.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for InstanceGate<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        let started = std::time::Instant::now();
        Box::pin(async move {
            // THE DECISION, about whichever of the two ways this route's input can name the
            // resource the request acts on the body actually uses. A body that is not
            // readable as JSON, or that names neither, gives the gate nothing to authorize:
            // there is nothing to resolve, so the request goes on to the modelled operation
            // and the framework's own refusal stands.
            let refusal = match named_instance(req) {
                Some(instance_id) => {
                    crate::state::ec2_fleet::instance_refusal(
                        &*self.engine,
                        ctx,
                        Some(instance_id.as_str()),
                    )
                    .await
                }
                // No instance named: the association an `InstanceUserArn` identifies is on
                // an instance too, so for a route that declares one THAT is what the request
                // names, and it is resolved against this account's own associations — in the
                // scope `ctx` identifies, which is why the context is threaded here exactly
                // as it is into the instance half above.
                None => crate::state::instance_user_refusal(
                    &*self.engine,
                    ctx,
                    named_resource_arn(req, self.route.arn_member).as_deref(),
                ),
            };
            let Some(refusal) = refusal else {
                // The modelled operation this route names, from this service's own
                // model-derived routing table (keyed by operation name).
                if let Some(operation) = svc.definition().routing_table().get(self.route.operation)
                {
                    return operation.run(svc, req, ctx).await;
                }
                // Unreachable while the model binds the route: the table is built from the
                // same model the route's path and operation come from. Answered as the
                // request naming no modelled operation, which is what it would be.
                return svc.definition().routing_error_response(req);
            };
            // The code travels to the telemetry row below as well as onto the wire, so it is
            // read off the refusal once rather than named again here.
            let code = refusal.code;
            let response = refusal_response(svc, req, &refusal);
            record_refusal(
                ctx,
                self.route.operation,
                code,
                response.status().as_u16(),
                started.elapsed(),
            );
            response
        })
    }

    // `None`, NOT an empty list: the request is still signed as the operation the model
    // names. Answering `Some(&[])` here would serve a gated request unsigned.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// The EC2 instance a gated request names, read off the RAW request body — before the
/// framework has deserialized or constraint-checked anything, which is the whole point of
/// deciding here.
///
/// `None` means the gate does not apply: the body is not readable as JSON, or it names no
/// `InstanceId` at all, so there is nothing to authorize.
fn named_instance(req: &aws_smithy_runtime_api::http::Request) -> Option<String> {
    body_member(req, "InstanceId")
}

/// The ARN a gated request addresses one of this service's own resources by, read off the
/// raw body exactly as [`named_instance`] reads the instance id, and for the same reason:
/// the decision is taken before the framework has looked at the request at all, so the ARN
/// is read here WHATEVER it says — a value outside the member's modelled `@pattern`
/// included, which is the case that would otherwise be refused as a constraint violation
/// before this decision was ever reached.
///
/// `None` when the route declares no such member, when the body is not readable as JSON, and
/// when it carries no value for the member — nothing is then named this way.
fn named_resource_arn(
    req: &aws_smithy_runtime_api::http::Request,
    member: Option<&'static str>,
) -> Option<String> {
    body_member(req, member?)
}

/// One top-level STRING member of the raw JSON request body, or `None` when the body is not
/// readable as JSON or carries no string there.
fn body_member(req: &aws_smithy_runtime_api::http::Request, member: &str) -> Option<String> {
    let body: serde_json::Value = serde_json::from_slice(req.body().bytes()?).ok()?;
    Some(body.get(member)?.as_str()?.to_string())
}

/// Record the refusal above as the operation row the framework's dispatch sequence would
/// have recorded, so a gated request is not a hole in the per-operation telemetry series:
/// answering from this seam skips that sequence, and the identical refusal the handler's
/// own gate answers IS recorded by it. Same classification (from the response's status and
/// the error code), same folded handle (this service's signing name and the operation the
/// request named), same already-sanitized request metadata, and gated on a client
/// collecting exactly as the framework gates it.
fn record_refusal(
    ctx: &OperationContext<'_>,
    operation: &str,
    code: &str,
    status: u16,
    latency: std::time::Duration,
) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle = emulator_telemetry::OperationHandle::folded(
        ctx.identity().signing_name_static(),
        operation,
    );
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            status,
            code,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}
