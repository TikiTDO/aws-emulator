// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sqs-amazonsqs` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself. Here it carries
//!   the transport's size guard: a request body above [`max_request_body_bytes`] is
//!   answered with a bare HTTP 413 instead of reaching a handler — and, for every other
//!   request, [`QueryCompatibleErrors`], which is NOT a re-route: the key and the handler
//!   stay the generated routing table's, and the wrapper only completes the error envelope
//!   this `awsQueryCompatible` service owes a client that asked for the legacy query code.
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

/// This front door's AUDIT leg: what this service contributes to the record of each
/// queue-level call it serves, and the routing table those calls are served through.
pub mod management_events;

use management_events::AuditedDefinition;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Refusal, Refused,
    Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The `com-amazonaws-sqs-amazonsqs` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, routing through this service's AUDITED operations: every
    /// queue-level call it serves is a management event of this service, recorded in the
    /// account's event history where CloudTrail's `LookupEvents` reads it and published
    /// onto the account's default event bus while a trail of the Region is logging calls
    /// of its kind (see [`management_events`]). Identity, the protocol and every framework
    /// error renderer stay the generated entry's.
    entry: AuditedDefinition<crate::ServiceDefinition<Eng>>,
    /// The transport's answer to a request body above [`max_request_body_bytes`], held
    /// here because [`Service::override_operation`] hands back a reference borrowed from
    /// `&self`.
    oversized: RequestEntityTooLarge,
    /// Every other request's modelled operation, served through the wrapper that completes
    /// this service's error envelope ([`QueryCompatibleErrors`]), held here for the same
    /// reason.
    query_compatible: QueryCompatibleErrors<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // Every modelled operation of this service, constructed from the same shared
        // engine: the queue-level ones served behind the audit recorder, so no control-plane
        // call reaches a handler by a path that records nothing, and the message data plane
        // filed beside them unrecorded (see [`management_events::audited_table`]).
        let audited = management_events::audited_table(&engine);
        Self {
            entry: AuditedDefinition::new(
                crate::ServiceDefinition::new(Arc::clone(&engine)),
                audited,
            ),
            oversized: RequestEntityTooLarge,
            query_compatible: QueryCompatibleErrors { engine },
        }
    }

    /// The wrapper that serves `req`'s own modelled operation and completes this service's
    /// error envelope, or `None` when the generated routing table names no operation for
    /// the request at all — which is the framework's routing failure and stays the
    /// framework's.
    fn serving_the_modelled_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.entry
            .routing_table()
            .get(key)
            .map(|_| &self.query_compatible as &dyn ErasedOperation)
    }
}

/// How much room beyond the payload the transport ceiling leaves for the request that
/// wraps it: the target and content-type framing, the queue url, the batch entry ids
/// (`DeleteMessageBatchRequestEntry$Id` is at most
/// [`crate::state::MAX_BATCH_ENTRY_ID_LEN`] characters, over at most
/// [`crate::state::MAX_BATCH_ENTRIES`] entries), the attribute names and types that
/// travel beside the bodies, and the protocol's own JSON punctuation and string escaping.
///
/// A fixed allowance rather than the sum of those modelled lengths: it only has to
/// comfortably exceed every member that can travel beside the payload, and a sum would
/// have to be re-derived whenever one more member is modelled. 64 KiB is the same figure
/// the only other per-service transport ceiling in this tree leaves for its own framing
/// (`service-com-amazonaws-codecommit-codecommit-20150413`'s `MAX_REQUEST_BODY_BYTES`).
const REQUEST_ENVELOPE_ALLOWANCE_BYTES: usize = 64 * 1024;

/// How far PAST the maximum total payload a request's message content may reach and still
/// be read.
///
/// A request that breaks the payload bound is one the OPERATION refuses, not the
/// transport: a batch whose entries are each individually acceptable and together carry
/// more than the maximum total payload is answered with the modelled
/// `BatchRequestTooLong`, which names the limit and the number of bytes the request sent.
/// That answer exists only if the request was READ and MEASURED, so the ceiling has to sit
/// above the payload limit by more than the envelope around it — an over-limit aggregate
/// is payload, and a client spells one by sending a handful of entries each well inside
/// the per-message bound (four entries of a third of the limit each add up to a third more
/// than it).
///
/// Amazon SQS publishes no figure for the request body its frontend reads, so how much
/// headroom is this emulator's own choice: half the payload limit again, which admits
/// every aggregate up to one and a half times the limit while a body several times it (a
/// single message of a few megabytes, ten maximum-size batch entries) is still refused on
/// the wire. Written as the divisor of the payload limit so the headroom tracks that limit
/// rather than being a second figure to maintain.
const OVER_LIMIT_PAYLOAD_HEADROOM_DIVISOR: usize = 2;

/// The largest HTTP request body this service reads at all.
///
/// The message content any Amazon SQS request may legally carry is ONE maximum total
/// payload ([`crate::state::maximum_batch_payload_bytes`]) — "the maximum allowed
/// individual message size and the maximum total payload size (the sum of the individual
/// lengths of all of the batched messages)" are the same figure, so a single message and a
/// whole batch are bounded alike. The ceiling is read from there rather than written out a
/// second time, and sits above it by two allowances: payload headroom
/// ([`OVER_LIMIT_PAYLOAD_HEADROOM_DIVISOR`]), which is what keeps the modelled
/// `BatchRequestTooLong` reachable — a batch whose entries add up to more than the payload
/// limit is still read, and refused by the OPERATION with the limit and the bytes it was
/// sent — and [`REQUEST_ENVELOPE_ALLOWANCE_BYTES`] for the request wrapped around that
/// payload.
///
/// A body larger than that sum carries more message content than any answer of this
/// service could even state, so it is refused by the TRANSPORT — a bare HTTP 413 with no
/// error document — before the body is deserialized and before any handler looks at a
/// field. That is a different refusal from a field-level one: a parameter refusal says the
/// service read the request and disliked a value, while this one says the request never fit
/// on the wire.
///
/// This is not a second copy of the framework's own 413. The core's two body caps
/// (`vendor/server/src/lib.rs`'s `MAX_PRODUCTION_BODY_BYTES`, 16 MiB on the serve path,
/// and `vendor/server/src/listener_guard.rs`'s `caps::MAX_BYTES_PER_CONN`, 64 MiB per
/// resource listener) bound how much the PROCESS will buffer for any service at all: both
/// are `const` with no per-service setting, and both sit orders of magnitude above any
/// Amazon SQS payload, so neither can state what this service accepts. The ceiling an
/// individual service reads at all is the service's own, which is why it lives here —
/// the same place `service-com-amazonaws-codecommit-codecommit-20150413` keeps the one
/// for its git transport.
pub fn max_request_body_bytes() -> usize {
    let payload = usize::try_from(crate::state::maximum_batch_payload_bytes()).unwrap_or(usize::MAX);
    payload
        .saturating_add(payload / OVER_LIMIT_PAYLOAD_HEADROOM_DIVISOR)
        .saturating_add(REQUEST_ENVELOPE_ALLOWANCE_BYTES)
}

/// The transport's refusal of a request whose body is above [`max_request_body_bytes`]:
/// HTTP 413 (Request Entity Too Large) and nothing else.
///
/// It is shaped as an operation only because the router names one before anything can be
/// answered; nothing of this service's model reaches it. It deserializes no input, takes
/// no store and writes nothing, so the oversized body is never read and no message it
/// carried is enqueued. The response carries NO body, so a client sees the bare transport
/// status rather than a service error code — which is the whole difference this refusal
/// states.
///
/// [`ErasedOperation::auth_schemes`] is deliberately left at its default `None`: an
/// author-written operation that answers `None` defers to the schemes the model names for
/// the operation the request's routing key resolves, so routing a request here never
/// weakens what must sign it.
#[derive(Debug)]
struct RequestEntityTooLarge;

/// The status the transport refuses an over-sized body with.
const REQUEST_ENTITY_TOO_LARGE: u16 = 413;

impl ErasedOperation for RequestEntityTooLarge {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        _req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            aws_smithy_runtime_api::http::Response::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(REQUEST_ENTITY_TOO_LARGE)
                    .expect("413 is a status code"),
                aws_smithy_types::body::SdkBody::empty(),
            )
        })
    }
}

/// The response header a service whose model carries `awsQueryCompatible` reports the
/// legacy query error code in, for a client that asked for it.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The request header a query-compatible SDK opts into that report with.
const QUERY_MODE_HEADER: &str = "x-amzn-query-mode";

/// The value that opt-in carries.
const QUERY_MODE_ON: &str = "true";

/// The fault side the report names for a refusal the CALLER caused — every 4xx.
const SENDER_FAULT: &str = "Sender";

/// The fault side it names for a failure the SERVICE caused — every 5xx.
const RECEIVER_FAULT: &str = "Receiver";

/// The lowest status that reports a failure rather than an answer, and so the one an error
/// class is reported for at all.
const FIRST_FAILURE_STATUS: u16 = 400;

/// The lowest status whose fault is the service's own.
const FIRST_SERVER_FAULT_STATUS: u16 = 500;

/// The modelled class this service words a refused attribute NAME with ([`refusal_for`]),
/// and — because the model defaults that shape's `awsQueryError` binding — the legacy query
/// code of the same class.
const UNKNOWN_ATTRIBUTE_NAME: &str = "InvalidAttributeName";

/// Every error class this service words a BOUNDARY refusal with: what [`refusal_for`] can
/// answer, which is also the only error a response carries no legacy code for by the time
/// it leaves the framework. Each entry is BOTH the shape name the renderer writes into
/// `__type` and the legacy query code of that class, because the model defaults the
/// `awsQueryError` binding of every shape named here.
const BOUNDARY_REFUSAL_CLASSES: &[&str] = &[UNKNOWN_ATTRIBUTE_NAME];

/// Whether `req` asked for the legacy query error report.
///
/// A query-compatible SDK sends the opt-in on every request it makes to this service; a
/// native JSON client sends nothing, and gets the unadorned JSON error. The report is
/// additive, so only the opt-in decides — nothing of the answer is withheld without it.
fn asked_for_the_query_error(req: &aws_smithy_runtime_api::http::Request) -> bool {
    req.headers()
        .get(QUERY_MODE_HEADER)
        .is_some_and(|value| value.eq_ignore_ascii_case(QUERY_MODE_ON))
}

/// The legacy query code the answer `resp` owes, when it is an error this service worded
/// at the BOUNDARY — and `None` for every other answer.
///
/// Only the boundary refusal is read here, and that is the whole point of reading a
/// response at all: every error the per-operation sequence renders states its own legacy
/// code through the error channel ([`crate::scaffolded`]'s `query_error_code`), so the
/// framework has already reported it. A refusal the constraint walk raised is rendered
/// OUTSIDE that sequence, from a code and a message alone ([`refusal_for`]), with no error
/// value left to ask — so the class is recovered from the `__type` the renderer wrote, and
/// only for the classes that seam words ([`BOUNDARY_REFUSAL_CLASSES`]).
///
/// A body naming anything else is left without a report rather than given a guessed one: the
/// framework's own refusal classes (its generic validation failure, the scaffold's 501) are
/// not error classes of this service's API, so they have no legacy code to name.
fn boundary_refusal_code(resp: &aws_smithy_runtime_api::http::Response) -> Option<&'static str> {
    let document: serde_json::Value = serde_json::from_slice(resp.body().bytes()?).ok()?;
    let shape = document.get("__type")?.as_str()?;
    let local = shape.split('#').next_back().unwrap_or(shape);
    BOUNDARY_REFUSAL_CLASSES
        .iter()
        .find(|class| **class == local)
        .copied()
}

/// The body member the JSON error renderers name the error shape in.
const TYPE_MEMBER: &str = "__type";

/// This service's own Smithy model, embedded from the file this crate is generated from.
///
/// The shape ids an error answer names its class under are read out of it, so the id a body
/// carries is the id the model declares and cannot drift from it. The include is a
/// COMPILE-TIME read of that file: a model that moves fails this crate's build.
const MODEL: &str = include_str!("../smithy-model/com-amazonaws-sqs-amazonsqs.json");

/// The fully qualified shape id this service's model declares for the error shape whose local
/// name is `local`, or `None` when its model declares no error shape of that name.
///
/// The map is built once per process, out of the model's own shapes: every shape it marks
/// with `@error`, under the local name the JSON renderer writes into `__type` and the
/// qualified id the model gives it. A name the model declares no error shape for — the
/// request-validation framework's own classes, which already name their own package, and the
/// scaffold's synthetic signal — is not one of this service's classes and is answered as the
/// layer that built it wrote it.
fn modelled_error_id(local: &str) -> Option<&'static str> {
    static IDS: std::sync::OnceLock<std::collections::BTreeMap<String, String>> =
        std::sync::OnceLock::new();
    IDS.get_or_init(|| {
        let shapes = serde_json::from_str::<serde_json::Value>(MODEL)
            .ok()
            .and_then(|document| document.get("shapes").cloned());
        let Some(serde_json::Value::Object(shapes)) = shapes else {
            return std::collections::BTreeMap::new();
        };
        shapes
            .iter()
            .filter(|(_, shape)| {
                shape.get("type").and_then(serde_json::Value::as_str) == Some("structure")
                    && shape
                        .get("traits")
                        .and_then(serde_json::Value::as_object)
                        .is_some_and(|traits| traits.contains_key("smithy.api#error"))
            })
            .filter_map(|(id, _)| {
                let name = id.split('#').next_back()?;
                (!name.is_empty()).then(|| (name.to_string(), id.clone()))
            })
            .collect()
    })
    .get(local)
    .map(String::as_str)
}

/// `resp` with the error class its body names stated in FULL — the qualified shape id this
/// service's model declares for it.
///
/// This is the other half of what `awsQueryCompatible` means for a service that migrated off
/// the query protocol: the body names the error SHAPE, and it names it by its shape id
/// (`com.amazonaws.sqs#QueueDoesNotExist`), which is what a modern client resolves its typed
/// exception from. The framework's JSON renderer writes the local shape name alone, because
/// that is all an error value carries, so the id is completed here — once, on the way out, so
/// every layer's failure names its class alike.
///
/// Left alone: a success (it names no class), a body naming a class of no shape of this
/// service's model, a body that already carries the qualified id, and every other member,
/// header and the status.
fn naming_the_class_in_full(
    mut resp: aws_smithy_runtime_api::http::Response,
) -> aws_smithy_runtime_api::http::Response {
    // An error body is built in memory by its renderer, so its bytes are readable here; a
    // streamed body is not an error body and is left alone.
    let Some(rewritten) = resp
        .body()
        .bytes()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        .and_then(qualified_type_document)
    else {
        return resp;
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        let _ = resp.headers_mut().try_insert("content-length", length);
    }
    *resp.body_mut() = aws_smithy_types::body::SdkBody::from(rewritten);
    resp
}

/// `document` with its `__type` restated as the model's own shape id, or `None` when there is
/// nothing to restate: a document naming no class, a class of no shape of this model, or one
/// already named in full.
fn qualified_type_document(mut document: serde_json::Value) -> Option<Vec<u8>> {
    let object = document.as_object_mut()?;
    let named = object.get(TYPE_MEMBER)?.as_str()?.trim().to_string();
    let shape_id = modelled_error_id(named.split('#').next_back().unwrap_or(&named))?;
    if named == shape_id {
        return None;
    }
    object.insert(TYPE_MEMBER.to_string(), serde_json::Value::from(shape_id));
    serde_json::to_vec(&document).ok()
}

/// `resp` as this `awsQueryCompatible` service answers it: the error class named in full in
/// the body ([`naming_the_class_in_full`]), and its legacy query code reported in the header
/// for a client that asked for one ([`reporting_the_query_error`]).
///
/// A success names no class and passes through untouched.
fn completing_the_envelope(
    req: &aws_smithy_runtime_api::http::Request,
    resp: aws_smithy_runtime_api::http::Response,
) -> aws_smithy_runtime_api::http::Response {
    if resp.status().as_u16() < FIRST_FAILURE_STATUS {
        return resp;
    }
    reporting_the_query_error(req, naming_the_class_in_full(resp))
}

/// `resp` with the legacy query error report added, for a request that asked for one.
///
/// `awsQueryCompatible` names an error class TWICE: the body's `__type` names the error
/// SHAPE, and this header reports the legacy query code of the same class beside the fault
/// side — `Sender` for a refusal the caller caused, `Receiver` for a failure this service
/// caused — which is the only place a client still speaking the query vocabulary can
/// recover the code from, since the body carries a shape id instead.
///
/// The framework's own per-operation sequence already writes this header from the error
/// value's own legacy code, so a response that carries one keeps it. What is left for this
/// to complete is the refusal the framework renders OUTSIDE that sequence
/// ([`boundary_refusal_code`]).
///
/// A success is left alone (there is no class to name), and so is a header the HTTP layer
/// will not carry: a report that could not be built costs the answer its header, never the
/// answer.
fn reporting_the_query_error(
    req: &aws_smithy_runtime_api::http::Request,
    mut resp: aws_smithy_runtime_api::http::Response,
) -> aws_smithy_runtime_api::http::Response {
    if resp.status().as_u16() < FIRST_FAILURE_STATUS
        || resp.headers().get(QUERY_ERROR_HEADER).is_some()
        || !asked_for_the_query_error(req)
    {
        return resp;
    }
    let Some(code) = boundary_refusal_code(&resp) else {
        return resp;
    };
    let fault = if resp.status().as_u16() >= FIRST_SERVER_FAULT_STATUS {
        RECEIVER_FAULT
    } else {
        SENDER_FAULT
    };
    let _ = resp
        .headers_mut()
        .try_insert(QUERY_ERROR_HEADER, format!("{code};{fault}"));
    resp
}

/// Serves the modelled operation the generated routing table names for the request,
/// exactly as that table would, and completes the error envelope this service owes on the
/// way out ([`completing_the_envelope`]).
///
/// It stores no operation of its own and makes no routing decision: the operation run is
/// the one the service's own table names for the request, so the framework still owns the
/// whole deserialize -> validate -> handle -> serialize sequence and every status, code and
/// message it answers with. The same wrapper serves EVERY operation, so a refusal raised
/// while ANY of them was being served — which the framework renders outside that sequence,
/// and so without the report — passes through one place that can complete it.
///
/// Being that one place, it is also where each queue's own `AWS/SQS` series are published:
/// the reading of its depth when one is due, and the counters the request contributed to
/// ([`crate::state::publish_queue_metrics`]), off the answer rather than in front of it. The
/// clock that keeps sampling the queues without a call is started from here
/// ([`crate::state::drive_queue_metric_clock`]).
#[derive(Debug)]
struct QueryCompatibleErrors<Eng: Send + Sync + 'static> {
    /// The SAME shared engine the generated entry threads into every operation, so the
    /// depths published are the ones this process's one store holds and they land in the one
    /// CloudWatch store beside it.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for QueryCompatibleErrors<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The definition is read and dropped inside this one statement: the operation
            // it hands back is `Send`, the `&dyn ErasedServiceDefinition` it was read from
            // is not, so holding it across the await below would make this future
            // non-`Send`.
            let modelled = {
                let definition = svc.definition();
                definition
                    .protocol()
                    .routing_key(req)
                    .ok()
                    .and_then(|key| definition.routing_table().get(key))
            };
            let response = match modelled {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable through `override_operation`, which hands a request here
                // only once the generated table has answered for its key; a request whose
                // key the table does not name is the framework's own routing failure,
                // worded by this service's entry exactly as it would be had nothing
                // claimed the key.
                None => svc.definition().routing_error_response(req),
            };
            let answered = completing_the_envelope(req, response);
            // The request has been answered; what it left the queues holding, and what it
            // contributed to their counters, is published now, with no store lock held, and
            // the clock that keeps sampling them is (re)started.
            crate::state::publish_queue_metrics(&*self.engine);
            crate::state::drive_queue_metric_clock(&self.engine);
            answered
        })
    }

    /// `None` is "defer to the modelled operation the routing key names" — which is the
    /// operation this wrapper delegates to, so its model-declared schemes decide and
    /// wrapping cannot weaken them.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// The error Amazon SQS names for a value the boundary validator refused, or `None` to
/// leave the framework's wording in place.
///
/// One case, and it is model-driven rather than keyed to a member name: a value
/// outside `QueueAttributeName`'s value set is an attribute name that does not exist,
/// which this service's model has an error shape for — `InvalidAttributeName`, "The
/// specified attribute doesn't exist." (HTTP 400, the same status the refusal already
/// carries). Every operation that takes attribute names reaches it through the same
/// enum, so matching on the refused value SET covers all of them without naming one.
///
/// An attribute name supplied as a MAP KEY (`CreateQueue`/`SetQueueAttributes`
/// `Attributes`) never arrives here: the constraint walk captures a map key rather than
/// validating it, so those handlers refuse an unknown name themselves
/// (`crate::state::validate_attributes`). This covers the LIST form
/// (`GetQueueAttributes` `AttributeNames`), which the walk does validate.
fn refusal_for(violation: &Violation) -> Option<Refused> {
    // The class below is the one [`BOUNDARY_REFUSAL_CLASSES`] holds, read from there so the
    // refusal and the legacy report of that same refusal cannot name two different classes.
    let class = UNKNOWN_ATTRIBUTE_NAME;
    let ViolationKind::EnumValue(allowed) = &violation.kind else {
        return None;
    };
    if allowed.len() != crate::ops::types::QUEUE_ATTRIBUTE_NAME_VALUES.len()
        || !allowed
            .iter()
            .all(|value| crate::ops::types::QUEUE_ATTRIBUTE_NAME_VALUES.contains(&value.as_str()))
    {
        return None;
    }
    let name = violation.value.as_deref().unwrap_or(&violation.path);
    Some(Refused {
        code: class.to_string(),
        message: format!("Unknown Attribute {name}."),
    })
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
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // BOUNDED BEFORE ANYTHING IS READ. The body reaching here is the one dispatch
        // will deserialize (the router already undid any `aws-chunked` framing), so this
        // is the first point at which its size is known and the last before a handler
        // reads a field off it. A body above the transport ceiling is answered by
        // [`RequestEntityTooLarge`] — the bare 413 — whatever operation it names, so no
        // modeled operation is handed a request the transport could not accept and
        // nothing it carried is stored. A body the ceiling admits routes exactly as it
        // did before.
        //
        // Every other request the generated table CAN serve resolves to
        // [`QueryCompatibleErrors`], which is not a re-route: it runs the very operation
        // that table names for the request's own key and only completes the error envelope
        // on the way out. A key the table does not name resolves to `None`, exactly as it
        // would in an unedited service, and falls through to the framework's routing
        // failure.
        match req.body().bytes() {
            Some(body) if body.len() > max_request_body_bytes() => Some(&self.oversized),
            Some(_) => self.serving_the_modelled_operation(req),
            // A body that is not in memory has no length to measure, so the ceiling
            // cannot be applied to it and this arm must not guess: it routes on. The
            // router buffers every request body before it looks an operation up, so the
            // arm is unreachable through dispatch, and that is what the suite in
            // `tests/all/request_size_transport_bound.rs` pins — a runtime that stopped
            // buffering would silently disable the bound, and those rows would fail.
            None => self.serving_the_modelled_operation(req),
        }
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Both the code and the message are replaced, because a
        // client branches on the code. Everything [`refusal_for`] does not recognise
        // keeps the framework's own wording.
        match refusal {
            Refusal::Invalid(violations) => violations.iter().find_map(refusal_for),
            // Authentication and every refusal added later keep the framework's
            // wording.
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

#[cfg(test)]
mod tests {
    use super::{modelled_error_id, qualified_type_document, TYPE_MEMBER};

    /// The class a body names is restated by the id this service's MODEL declares for it, and
    /// nothing else is restated — one row per kind of name a layer of this service can write
    /// into the discriminator.
    #[test]
    fn a_class_of_this_services_model_is_named_by_its_shape_id() {
        // (case, the name a renderer wrote, the id the body then carries)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a modelled class the model binds a legacy code to",
                "QueueDoesNotExist",
                Some("com.amazonaws.sqs#QueueDoesNotExist"),
            ),
            (
                "a modelled class whose legacy binding the model defaults",
                "InvalidAttributeValue",
                Some("com.amazonaws.sqs#InvalidAttributeValue"),
            ),
            (
                "a modelled class of the batch operations",
                "TooManyEntriesInBatchRequest",
                Some("com.amazonaws.sqs#TooManyEntriesInBatchRequest"),
            ),
            (
                "the same class already named in full",
                "com.amazonaws.sqs#QueueDoesNotExist",
                None,
            ),
            (
                "the request-validation framework's own class",
                "com.amazon.coral.service#InvalidParameterValueException",
                None,
            ),
            ("the scaffold's own signal", "NotImplemented", None),
            ("a name of no shape of this model", "ValidationException", None),
        ];
        for (case, written, expected) in rows {
            let document = ::serde_json::json!({ TYPE_MEMBER: written, "message": "no" });
            let rewritten = qualified_type_document(document).map(|bytes| {
                ::serde_json::from_slice::<::serde_json::Value>(&bytes)
                    .expect("the rewritten body is JSON")[TYPE_MEMBER]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            });
            assert_eq!(
                rewritten.as_deref(),
                *expected,
                "case {case:?}: expected the body to name {expected:?}, got {rewritten:?}"
            );
        }
    }

    /// The ids come from the model rather than from this file: a shape it does not mark as an
    /// error is no class of an answer, whatever its name looks like.
    #[test]
    fn only_the_models_error_shapes_are_classes() {
        for local in ["QueueDoesNotExist", "QueueNameExists", "InvalidMessageContents"] {
            assert_eq!(
                modelled_error_id(local),
                Some(format!("com.amazonaws.sqs#{local}").as_str()),
                "case {local:?}: expected the model's own id for the error shape"
            );
        }
        for local in ["SendMessageRequest", "QueueAttributeName", "Message", ""] {
            assert_eq!(
                modelled_error_id(local),
                None,
                "case {local:?}: this names no error shape of the model, so no answer names a \
                 class by it"
            );
        }
    }
}
