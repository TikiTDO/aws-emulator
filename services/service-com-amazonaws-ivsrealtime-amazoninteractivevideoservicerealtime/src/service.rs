// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE. The key and the handler stay the generated
//!   table's; each operation is served through [`StatedEnvelope`], which states a
//!   constraint refusal the way IVS RealTime states it and names the request every
//!   response answers. See "The envelope every response carries" below.
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
//! # The envelope every response carries
//!
//! AWS answers every IVS RealTime call — success or failure — with a request id, and
//! answers a failure with the error's code and text in the places a client reads them.
//! The model says which places: `cors.additionalExposedHeaders`
//! (`smithy_query(query="service_info")`) lists `x-amzn-requestid`, `x-amzn-errortype`
//! and `x-amzn-errormessage`, which are the three response headers this service publishes
//! to a browser client and therefore the three it sends. The protocol serializer already
//! writes `x-amzn-errortype` (and the body's `__type`) from the modelled error shape, so
//! what is added here is the request id on every response and the error text beside the
//! code on a failure.
//!
//! The id is a property of the RESPONSE, not of any modelled shape: no operation's output
//! declares it, so no handler can carry it, and a failure must carry one exactly as a
//! success does. Hence [`with_response_envelope`], applied at the one place every
//! response of this service passes through — [`StatedEnvelope`], the per-operation
//! wrapper — and on the three framework-level renderers the operations never reach
//! ([`RealTimeDefinition`]).
//!
//! The CORS and security headers of `crate::state::RESPONSE_HEADER_POLICY` travel the same
//! way on a FAILURE, and only there. The model binds those members on four output shapes
//! and on every error shape it declares, so a handler carries them on a success
//! (`crate::state::ResponseHeaderPolicy`) and the protocol serializer writes them from the
//! model's own `@httpHeader` traits; a failure is rendered as the one-member body
//! `crate::scaffolded` describes, whose members the serializer never sees, so the headers
//! are written onto the refusal here instead. The set is the same one a success carries, so
//! the policy a client reads off this endpoint does not depend on the outcome of the call —
//! including the cross-origin read grant, which names which origins may read an answer and
//! admits no request this service would otherwise refuse.
//!
//! # How this service words a refusal
//!
//! IVS RealTime states a value outside a modelled constraint as `<Operation>.<member
//! path>` followed by the constraint it violated — "`CreateParticipantToken.duration is
//! required to have a maximum value of 20160.0, found 20161.0`" — where the framework's
//! own pre-dispatch envelope words every service's refusal alike ("1 validation error
//! detected: Value '…' at '…' failed to satisfy constraint: …"), with no operation in it.
//!
//! The operation is not reachable at the framework's refusal seam:
//! [`Service::override_refusal`] is handed the violations alone, with no request and no
//! operation. So the operations are served through [`StatedEnvelope`], which collects the
//! refusal itself — from the framework's own deserializer and constraint walk, so not one
//! modelled constraint is restated here — and words each violation from the OPERATION it
//! refused, which a per-operation wrapper has in scope as `O::NAME`. The sentences live in
//! `crate::state`, beside the message rules the handlers state their modelled failures
//! with, and the body is the one key an SDK error parser reads
//! (`crate::scaffolded::StatedBody`). Nothing else about the request changes: a request
//! this service does not refuse is handed to the generated `ErasedOperation::run`, which
//! owns deserialize -> validate -> handle -> serialize, telemetry, and every modelled
//! error.
//!
//! Two costs of serving the operations this way are deliberate. The wrapper reads the
//! request a second time (`deserialize_input` + `constraints::validate`, which the
//! generated `run` then repeats), so a request the model accepts is parsed and validated
//! twice; the framework exposes no seam that hands a refusal to the service before
//! rendering it, and the correct refusal is worth one extra parse of a control-plane
//! request. And the wrapped set is every operation the model binds, spelled out in
//! [`EmulatedService::new`]: an operation the model gains later is NOT wrapped until it is
//! listed there, and keeps the framework's generic sentence and no request id until it is —
//! the fallback is a worse envelope, never a wrong route, because `override_operation`
//! answers `None` for a key it does not hold and the generated table serves it unchanged.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema_server::constraints;
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, ErasedServiceDefinition, Operation,
    OperationContext, OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service,
    ServiceIdentity, Violation,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

/// The wire error code for a request this service refuses as invalid: the shape
/// `com.amazonaws.ivsrealtime#ValidationException`, which this service declares as a
/// common error of every operation (`smithy_query(query="service_info")` —
/// `common_errors`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The HTTP status that shape's `@httpError` declares.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// The response header AWS names the request it answers in, for this protocol, and one of
/// the three this service's model publishes in `cors.additionalExposedHeaders`.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The response header the protocol serializer writes an error's CODE in, and the one
/// restJson1 resolves an error shape from. Read here (never written) to fill the error
/// document's own code member, so the code in the document is the model-derived one and
/// not a second derivation of it.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The response header AWS states an error's text in, beside the code. Also from
/// `cors.additionalExposedHeaders`.
const ERROR_MESSAGE_HEADER: &str = "x-amzn-errormessage";

/// The body key an SDK error parser reads an error's text under, and the one
/// `crate::scaffolded::STATED_ERROR_SCHEMA` renders it as.
const SDK_MESSAGE_KEY: &str = "message";

/// The body key the protocol serializer writes the error shape name under.
const SDK_TYPE_KEY: &str = "__type";

/// The three members of the error document this service answers a failure with: the
/// error's code, its text, and the id of the request it answers.
///
/// Recorded: a refused IVS RealTime call answers a JSON document carrying `Code`,
/// `Message` and `RequestID`, which is what a client of this service reads a failure out
/// of. They sit beside the protocol serializer's own `__type` and the
/// [`SDK_MESSAGE_KEY`] an SDK error parser reads — the same code and the same text under
/// the names each reader looks for, never a second or different rendering of either.
const DOCUMENT_CODE_KEY: &str = "Code";
const DOCUMENT_MESSAGE_KEY: &str = "Message";
const DOCUMENT_REQUEST_ID_KEY: &str = "RequestID";

/// The lowest status that makes a response a failure, and so the responses the error
/// document belongs on.
const FIRST_ERROR_STATUS: u16 = 400;

/// The status whose response carries NO content, whatever a renderer wrote for it, and
/// the length such an answer states.
///
/// The two operations that answer it (`TagResource` and `UntagResource` — see
/// [`crate::state::NO_CONTENT_SUCCESS_STATUS`]) declare an output STRUCTURE with no
/// members rather than `smithy.api#Unit`, and the protocol serializes that as the empty
/// JSON object `{}`. The recorded responses carry no body at all, so the body is dropped
/// here — at the one place every response of this service passes through, rather than per
/// operation — and the answer still states its length, which is what the recorded
/// empty-bodied AWS responses do (the sibling `schemas` emulator records the same
/// behaviour for its own empty 204s). An answer served over HTTP/1.1 reaches the client
/// without the header: the HTTP writer the vendored transport uses removes
/// `content-length` from a 204, which is below anything this crate decides.
const NO_CONTENT_STATUS: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// Mint the id ONE response names the request it answers with.
///
/// 8-4-4-4-12 lowercase hex, the form an AWS request id takes. The VALUE is per-response
/// and is not a requirement: nothing can predict it and no caller may depend on a
/// particular one. What it must do is name ONE response, so it is drawn from a per-process
/// counter run through [`scramble`] — a bijection, so two responses never carry the same
/// id — mixed with the instant the response was answered, so two runs of this emulator do
/// not mint the same sequence. It is an identifier for correlation, never a secret:
/// nothing authenticates with it, and nothing the REQUEST carried goes into it.
///
/// A free function rather than a mint on the store: a failure the request boundary refused
/// never reaches a handler, so the envelope that answers it has no state to reach.
fn new_request_id() -> String {
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let instant = since_epoch
        .as_secs()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(u64::from(since_epoch.subsec_nanos()));
    // The high half is a bijection of the counter alone, which is what makes two ids
    // minted in the same nanosecond differ; the low half carries the instant.
    let high = scramble(counter);
    let low = scramble(instant ^ scramble(counter.wrapping_add(1)));
    let hex = format!("{high:016x}{low:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Mix a counter so consecutive values render as unrelated ids.
///
/// SplitMix64's finaliser applied to the counter times the 64-bit golden ratio. Every
/// step (multiply by an odd constant, xor-shift) is a bijection on `u64`, so the whole
/// function is one: distinct counters never collide.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The error document this service answers `resp` with, when `resp` is a failure carrying
/// a JSON body: the document the renderer already wrote, with the code, the text and the
/// request id added under the names a client of this service reads them by.
///
/// NOTHING is derived twice. The code is the one the protocol serializer put in
/// [`ERROR_TYPE_HEADER`] — model-derived, from the modelled error shape the operation
/// declares — falling back to the body's own `__type` when a renderer wrote only that.
/// The text is the one the renderer wrote under [`SDK_MESSAGE_KEY`], built by
/// `crate::state`'s message rules. A member the document already carries is left alone.
///
/// `None` when there is nothing to restate: a success, a response with no body (the
/// modelled 204 of `DeleteStage`, say), a streamed body, a body that is not a JSON
/// object, or an error whose code cannot be read at all — in which case the response goes
/// out exactly as the renderer wrote it rather than with a document this service made up.
fn error_document(resp: &RtResponse, request_id: &str) -> Option<RestatedError> {
    if resp.status().as_u16() < FIRST_ERROR_STATUS {
        return None;
    }
    // An error body is built in memory by its renderer, so the bytes are readable here;
    // a streamed body is not an error document and is left alone.
    let bytes = resp.body().bytes()?;
    let mut document: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(bytes).ok()?;
    let code = resp
        .headers()
        .get(ERROR_TYPE_HEADER)
        .map(str::to_string)
        .or_else(|| {
            document
                .get(SDK_TYPE_KEY)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })?;
    let message = document
        .get(SDK_MESSAGE_KEY)
        .or_else(|| document.get(DOCUMENT_MESSAGE_KEY))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    document
        .entry(DOCUMENT_CODE_KEY)
        .or_insert_with(|| serde_json::Value::String(code));
    if !message.is_empty() {
        document
            .entry(DOCUMENT_MESSAGE_KEY)
            .or_insert_with(|| serde_json::Value::String(message.clone()));
    }
    document
        .entry(DOCUMENT_REQUEST_ID_KEY)
        .or_insert_with(|| serde_json::Value::String(request_id.to_string()));
    Some(RestatedError {
        body: serde_json::Value::Object(document).to_string(),
        message,
    })
}

/// One failure's error document and the text it states, so the header and the body carry
/// the same text and not two renderings of it.
struct RestatedError {
    body: String,
    message: String,
}

/// The response as this service sends it: naming the request it answers, and — on a
/// failure — stating the error's code, text and request id in the document a client reads
/// a failure out of.
///
/// The status and every member the renderer wrote are left exactly as they were, and a
/// header already present is left alone, so the one writer of each is whoever wrote it
/// first. Every header insert is FALLIBLE: a header the HTTP layer cannot represent is
/// dropped rather than turned into a transport failure, which on the error path would
/// replace a modelled 4xx with a 500 an SDK retries.
fn with_response_envelope(mut resp: RtResponse) -> RtResponse {
    let request_id = new_request_id();
    if resp.status().as_u16() == NO_CONTENT_STATUS {
        // See `NO_CONTENT_STATUS`: the body the serializer rendered for an empty output
        // structure goes no further, and the answer states the length it does have.
        *resp.body_mut() = aws_smithy_types::body::SdkBody::empty();
        let _ = resp
            .headers_mut()
            .try_insert("content-length", NO_CONTENT_LENGTH);
    }
    if let Some(restated) = error_document(&resp, &request_id) {
        // Only a renderer that advertised a length needs it corrected; the framework's
        // error renderers set `content-type` alone and leave framing to the transport.
        if resp.headers().get("content-length").is_some() {
            let length = restated.body.len().to_string();
            let _ = resp.headers_mut().try_insert("content-length", length);
        }
        *resp.body_mut() = aws_smithy_types::body::SdkBody::from(restated.body);
        if resp.headers().get(ERROR_MESSAGE_HEADER).is_none() && !restated.message.is_empty() {
            let _ = resp
                .headers_mut()
                .try_insert(ERROR_MESSAGE_HEADER, restated.message);
        }
    }
    if resp.headers().get(REQUEST_ID_HEADER).is_none() {
        let _ = resp.headers_mut().try_insert(REQUEST_ID_HEADER, request_id);
    }
    if resp.status().as_u16() >= FIRST_ERROR_STATUS {
        // Every error shape this model declares binds the same CORS and security headers
        // its four header-bearing output shapes do. A SUCCESS carries them as the modelled
        // output members `crate::state::ResponseHeaderPolicy` fills in, so the serializer
        // has already written them from the model's own `@httpHeader` traits and they are
        // found present here; a FAILURE renders the one-member body `crate::scaffolded`
        // describes rather than the modelled error shape, so its members never reach the
        // serializer and the headers are written onto the response here, by the same
        // `crate::state` policy the outputs report.
        for (name, value) in crate::state::RESPONSE_HEADER_POLICY {
            if resp.headers().get(name).is_none() {
                let _ = resp.headers_mut().try_insert(name, value);
            }
        }
    }
    resp
}

/// Whether the request these `parts` came from matches this service's SigV4 signing
/// name AND the modelled URI path space its generated `ROUTE_PATH_SEGMENTS` declares.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of that rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment must
/// be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact segment
/// equality. That path conjunct is what separates two services sharing one SigV4 signing
/// name — this RealTime endpoint (`ivsrealtime`) and the low-latency IVS endpoint
/// (`ivs`) both sign as `ivs`, so the key alone matches both and the router rejects every
/// such request as `500 AmbiguousService`.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed only
/// the request's parts, so the two things it reads — the URI and the headers — are
/// re-presented to it as a bodiless request. It reads neither the body nor the method, so
/// nothing it decides on is missing.
fn claims_route_key_and_path(parts: &http::request::Parts) -> bool {
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

/// The one first URI path segment the two services that sign as `ivs` BOTH bind, so the
/// model-derived path discriminator alone cannot separate them there.
///
/// Each of the two models binds its three tagging operations to `/tags/{resourceArn}`
/// (`ListTagsForResource` `GET`, `TagResource` `POST`, `UntagResource` `DELETE`); every
/// other operation of either service binds a URI whose first segment is its own
/// operation name, and those are disjoint. Source: `smithy_query(query="digest",
/// model_path="services/service-com-amazonaws-ivsrealtime-\
/// amazoninteractivevideoservicerealtime/smithy-model/\
/// com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime.json")` and the same
/// call on the `…-ivs-amazoninteractivevideoservice` model.
const SHARED_TAGGING_PATH_SEGMENT: &str = "tags";

/// The ARN resource types THIS endpoint owns — the resources its own model creates, and
/// therefore the only ones its tagging operations can address.
///
/// Both services' ARNs live in the one `ivs` arnNamespace, so the resource TYPE is what
/// names the owning endpoint. Source: this model's ARN shapes, whose patterns pin the
/// resource segment — `smithy_query(query="shape",
/// shape="CompositionArn,EncoderConfigurationArn,IngestConfigurationArn,PublicKeyArn,\
/// StageArn,StorageConfigurationArn",
/// model_path="services/service-com-amazonaws-ivsrealtime-\
/// amazoninteractivevideoservicerealtime/smithy-model/\
/// com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime.json")` — one per
/// resource this service's `Create…`/`Import…`/`StartComposition` operations mint. This
/// model's `ChannelArn` is deliberately NOT here: it is a REFERENCE to a low-latency
/// channel (this service has no operation that creates one), and a channel is tagged
/// through the `ivs` endpoint.
const OWN_RESOURCE_TYPES: &[&str] = &[
    "composition",
    "encoder-configuration",
    "ingest-configuration",
    "public-key",
    "stage",
    "storage-configuration",
];

/// The `arnNamespace` both services share, and so the only service field an ARN either
/// of their tagging operations addresses can carry. From
/// `smithy_query(query="service_info")` on either model (`awsService.arnNamespace`:
/// `ivs` for both).
const SHARED_ARN_NAMESPACE: &str = "ivs";

/// What a request path says about the `/tags/{resourceArn}` space both `ivs`-signing
/// services publish.
#[derive(Debug)]
enum TaggingTarget {
    /// Not on the shared path at all: the modelled path space has already decided the
    /// claim, and this discriminator adds nothing.
    NotShared,
    /// On the shared path, naming an IVS resource of this type (lowercased).
    Resource(String),
    /// On the shared path, naming no IVS resource either model could recognise.
    Unrecognised,
}

/// Read the shared tagging path: whether this request is on it, and which IVS resource
/// type its `{resourceArn}` label names.
///
/// One function rather than a path-segment helper plus an ARN helper, so the whole
/// shared-path decision is read in one place. The `{resourceArn}` label is
/// percent-decoded (it is a non-greedy `@httpLabel`, so a client encodes its `:` and
/// `/`) and its resource type is lowercased, so a non-canonical ARN cannot land a
/// request on the wrong endpoint. The `tags` segment itself is compared RAW, because
/// the framework's path conjunct has already matched that segment against the model's
/// `ROUTE_PATH_SEGMENTS` by exact equality: decoding it here could only disagree with
/// the conjunct that runs first.
///
/// The label counts as naming a resource only when it is an ARN in the `ivs` namespace
/// the two services share — `arn:<partition>:ivs:<region>:<account>:<type>/<id>`, the
/// form every ARN shape in either model pins — so another service's ARN, or a string
/// that merely has a sixth `:`-field, names nothing. Reading that field and cutting it
/// at `/` also handles a client that sent the ARN's separators raw, because the ARN's
/// own `/` then just starts the next path segment.
fn tagging_target(path: &str) -> TaggingTarget {
    let mut segments = path.trim_start_matches('/').split('/');
    if segments.next() != Some(SHARED_TAGGING_PATH_SEGMENT) {
        return TaggingTarget::NotShared;
    }
    let Some(label) = segments.next() else {
        return TaggingTarget::Unrecognised;
    };
    let arn = smithy_server_runtime::percent_decode(label);
    let fields: Vec<&str> = arn.split(':').collect();
    // arn : <partition> : <service> : <region> : <account> : <resource-type>/<id>
    let [prefix, _partition, service, region, account, resource, ..] = fields.as_slice() else {
        return TaggingTarget::Unrecognised;
    };
    let kind = resource
        .split('/')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !prefix.eq_ignore_ascii_case("arn")
        || !service.eq_ignore_ascii_case(SHARED_ARN_NAMESPACE)
        || region.is_empty()
        || account.is_empty()
        || kind.is_empty()
    {
        return TaggingTarget::Unrecognised;
    }
    TaggingTarget::Resource(kind)
}

/// Whether this service claims a request on the `/tags/{resourceArn}` path space both
/// `ivs`-signing services publish.
///
/// The ARN in the path names the resource, and a resource belongs to exactly one of the
/// two endpoints, so the resource type decides: this endpoint claims a tagging request
/// only when the ARN names one of ITS resources. Everything else — including a label
/// that names no recognisable IVS resource — is left to the sibling `ivs` endpoint, whose
/// `endpointPrefix` is the shared signing name, so exactly one service claims every
/// request on this path.
fn claims_shared_tagging_path(parts: &http::request::Parts) -> bool {
    match tagging_target(parts.uri.path()) {
        TaggingTarget::NotShared => true,
        TaggingTarget::Unrecognised => false,
        TaggingTarget::Resource(kind) => OWN_RESOURCE_TYPES.contains(&kind.as_str()),
    }
}

/// The `com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers.
    entry: RealTimeDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`StatedEnvelope`] so a constraint refusal is stated the way IVS RealTime states it
    /// and every response names the request it answers. Keyed by the routing key the
    /// protocol resolves — the key the generated table uses — and built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
    wrapped: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`StatedEnvelope`] and keyed by the operation's own `Operation::NAME` — the
/// model-derived routing key `ServerHttpProtocol::routing_key` resolves and the
/// generated table is keyed by, so no key is spelled by hand here.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(StatedEnvelope::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut wrapped = OperationRoutingTable::new();
        // EVERY operation the model binds. A declared constraint is a property of the
        // INPUT SHAPE, not of a handler, and a request id is a property of the RESPONSE,
        // so the wrapper goes in front of all of them — implemented or still scaffolded:
        // a request that violates the model is refused the same way whoever would have
        // served it, and a 501 names the request it answers like every other response.
        wrap_operations!(
            wrapped,
            engine,
            create_encoder_configuration::CreateEncoderConfiguration,
            create_ingest_configuration::CreateIngestConfiguration,
            create_participant_token::CreateParticipantToken,
            create_stage::CreateStage,
            create_storage_configuration::CreateStorageConfiguration,
            delete_encoder_configuration::DeleteEncoderConfiguration,
            delete_ingest_configuration::DeleteIngestConfiguration,
            delete_public_key::DeletePublicKey,
            delete_stage::DeleteStage,
            delete_storage_configuration::DeleteStorageConfiguration,
            disconnect_participant::DisconnectParticipant,
            get_composition::GetComposition,
            get_encoder_configuration::GetEncoderConfiguration,
            get_ingest_configuration::GetIngestConfiguration,
            get_participant::GetParticipant,
            get_public_key::GetPublicKey,
            get_stage::GetStage,
            get_stage_session::GetStageSession,
            get_storage_configuration::GetStorageConfiguration,
            import_public_key::ImportPublicKey,
            list_compositions::ListCompositions,
            list_encoder_configurations::ListEncoderConfigurations,
            list_ingest_configurations::ListIngestConfigurations,
            list_participant_events::ListParticipantEvents,
            list_participant_replicas::ListParticipantReplicas,
            list_participants::ListParticipants,
            list_public_keys::ListPublicKeys,
            list_stage_sessions::ListStageSessions,
            list_stages::ListStages,
            list_storage_configurations::ListStorageConfigurations,
            list_tags_for_resource::ListTagsForResource,
            start_composition::StartComposition,
            start_participant_replication::StartParticipantReplication,
            stop_composition::StopComposition,
            stop_participant_replication::StopParticipantReplication,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_ingest_configuration::UpdateIngestConfiguration,
            update_stage::UpdateStage,
        );
        Self {
            entry: RealTimeDefinition::new(engine),
            wrapped,
        }
    }
}

/// One modelled operation, served with the constraint refusals it owes stated the way IVS
/// RealTime states them — the OPERATION, the dotted member path, and the constraint the
/// request violated — and with the envelope every response of this service carries.
///
/// The refusal is built from the structured [`Violation`]s the framework's own walk
/// collects, never from a rendered response and never from a constraint restated here, so
/// the wrapper cannot find a different set of violations from the walk that follows it.
/// What the wrapper adds is the one thing that walk cannot know: which operation was
/// refused, which a per-operation wrapper has as `O::NAME`.
struct StatedEnvelope<O> {
    operation: O,
}

impl<O> StatedEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for StatedEnvelope<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition` is
            // not `Sync`, and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            let violations = refused_by::<O>(protocol, req);
            // The one modelled constraint of this service whose refusal is NOT this
            // service's constraint wording: the tagging surface's own bound on a
            // resource's tag set, which that surface answers in its own code and words
            // wherever the count is found (see `crate::state::check_tag_count`).
            if crate::state::is_tag_limit_violation(O::NAME, &violations) {
                if let Some(refusal) = tag_limit_exceeded(protocol) {
                    return with_response_envelope(refusal);
                }
            }
            if let Some(refusal) = validation_exception(protocol, O::NAME, &violations) {
                return with_response_envelope(refusal);
            }
            with_response_envelope(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Every violation that refuses `req` before `O`'s handler runs: a `@required` member
/// absent, or a value outside a constraint the model declares (`@length`, `@range`,
/// `@pattern`, an enum's value set, `@uniqueItems`). Empty when the request satisfies
/// the model, and empty for a failure that is NOT a refusal — a body the codec cannot
/// read at all is a serialization failure, which the framework words per protocol.
///
/// Both classes come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input), which is the same pair the centralized
/// sequence uses, so no rule is restated here.
fn refused_by<O: Operation>(protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Vec<Violation> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return Vec::new();
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => constraints::validate(&input, O::INPUT_SCHEMA),
        // Absence is found while READING the request and a bad value while walking what
        // was read; to a caller they are one refusal, so they are one here — exactly as
        // `smithy_server_runtime::refusal_of` classifies them.
        Err(SerdeError::MissingMember { member_name }) => {
            vec![crate::state::missing_member_violation(member_name)]
        }
        Err(_) => Vec::new(),
    }
}

/// This service's answer to `violations` for `operation`: HTTP 400 `ValidationException`
/// with the prose under the body key an SDK error parser reads.
///
/// `None` when there is nothing to refuse — and also when the refusal cannot be
/// serialized, which leaves the request to the framework's walk rather than answering a
/// 400 with no reason at all.
fn validation_exception(
    protocol: &dyn ServerHttpProtocol,
    operation: &str,
    violations: &[Violation],
) -> Option<RtResponse> {
    let refused = crate::scaffolded::StatedBody::new(crate::state::stated_violations(
        operation, violations,
    )?);
    protocol
        .serialize_error(
            &refused,
            &crate::scaffolded::STATED_ERROR_SCHEMA,
            VALIDATION_EXCEPTION,
            VALIDATION_EXCEPTION_STATUS,
        )
        // A refusal that will not serialize leaves the request to the framework, which
        // refuses it in the generic envelope — the very divergence this wrapper removes,
        // so it is logged rather than passed over silently.
        .inspect_err(|error| {
            tracing::warn!(
                %error,
                "the stated ValidationException would not serialize; this refusal falls \
                 back to the framework's generic envelope"
            );
        })
        .ok()
}

/// The tagging surface's answer to a request whose own `tags` map is already past the
/// modelled `Tags` maximum: the code and the sentence that surface states the bound with
/// (`crate::state::tagging_bad_request`), rather than this service's constraint wording.
///
/// The bound belongs to the RESOURCE's tag set, so a handler answers it for a call whose
/// composition would exceed it (`crate::state::check_tag_count`); a request carrying more
/// entries than the bound by itself never reaches that handler, because the framework's
/// walk refuses the map first. Both are the one bound, so both are answered in the one
/// code and the one sentence.
///
/// `None` when the refusal cannot be serialized, which leaves the request to
/// [`validation_exception`] rather than answering with no reason at all.
fn tag_limit_exceeded(protocol: &dyn ServerHttpProtocol) -> Option<RtResponse> {
    let refused = crate::scaffolded::StatedBody::new(crate::state::tag_limit_exceeded_message());
    protocol
        .serialize_error(
            &refused,
            &crate::scaffolded::STATED_ERROR_SCHEMA,
            crate::state::TAGGING_BAD_REQUEST_CODE,
            crate::state::TAGGING_BAD_REQUEST_STATUS,
        )
        .inspect_err(|error| {
            tracing::warn!(
                %error,
                "the tagging surface's tag-limit refusal would not serialize; this \
                 refusal falls back to the stated ValidationException"
            );
        })
        .ok()
}

/// This service's model-derived half, with the framework-level error renderers wrapped so
/// a routing failure, a transport failure and a pre-dispatch refusal answer in the same
/// envelope every other response does.
///
/// Identity, the routing table and the protocol come from the generated
/// [`crate::ServiceDefinition`] untouched — nothing model-derived is restated here.
struct RealTimeDefinition<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> RealTimeDefinition<Eng> {
    /// Build the definition from the shared engine, which the generated entry threads
    /// into every operation.
    fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }

    /// The routing key this request resolves to under this service's protocol.
    fn routing_key<'a>(&self, req: &'a RtRequest) -> Option<&'a str> {
        self.entry.protocol().routing_key(req).ok()
    }
}

impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for RealTimeDefinition<Eng> {
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
        with_response_envelope(self.entry.routing_error_response(req))
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        with_response_envelope(self.entry.framework_error_response(error, req))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        with_response_envelope(ErasedServiceDefinition::refusal_response(
            &self.entry,
            error,
            chosen,
            req,
        ))
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.entry.wire_protocol(req)
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
        // This service signs as `ivs` — a signing name it SHARES with the low-latency IVS
        // service (`services/service-com-amazonaws-ivs-amazoninteractivevideoservice`),
        // whose endpoint is `ivs` while this one's is `ivsrealtime`. The router resolves a
        // request's routing key from its SigV4 credential scope first, so BOTH services
        // see the key `ivs`: claiming on the signing name alone makes each one claim the
        // other's traffic, which the router answers `500 AmbiguousService` instead of
        // serving either.
        //
        // So the claim is narrowed to this service's own request space, in the two steps
        // the request itself can be read by: its modelled URI path space
        // (`claims_route_key_and_path`), and — for the one path space both models publish,
        // `/tags/{resourceArn}` — the resource the ARN in that path names
        // (`claims_shared_tagging_path`).
        claims_route_key_and_path(parts) && claims_shared_tagging_path(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `StatedEnvelope` so a constraint refusal is stated the way IVS RealTime states
        // it (naming the operation and the member path) rather than in the framework's
        // pre-dispatch envelope, and so every response names the request it answers. A
        // key the model does not name resolves to `None` here exactly as it does in the
        // generated table, so the router's routing-error path is unchanged.
        let key = self.entry.routing_key(req)?;
        self.wrapped.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // `StatedEnvelope` states a constraint refusal itself, from the operation that
        // was refused, so this seam is only reached for a refusal NO operation owns —
        // one the router answers before a key is resolved (an unauthenticated caller),
        // or the serialization fallback in `validation_exception`. Neither carries an
        // operation to name, and this service's sentence is built from one, so this keeps
        // the framework's wording rather than inventing a sentence with the operation
        // missing from it. The response still carries the envelope, through
        // `RealTimeDefinition::refusal_response`.
        let _ = refusal;
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders this service's own
        // `RealTimeDefinition::routing_error_response`, which is the framework's
        // protocol-correct default plus the response envelope.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps this service's own
        // `RealTimeDefinition::framework_error_response`: the entry's protocol-correct
        // rendering plus the response envelope. The error is borrowed, so the default
        // renderer can still consume it on `None`.
        let _ = error;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two responses never name the same request, and an id takes the 8-4-4-4-12
    /// lowercase-hex form an AWS request id takes.
    #[test]
    fn each_minted_request_id_is_fresh_and_shaped() {
        let mut seen: Vec<String> = Vec::new();
        for case in 0..64 {
            let id = new_request_id();
            let groups: Vec<&str> = id.split('-').collect();
            assert_eq!(
                groups.iter().map(|g| g.len()).collect::<Vec<usize>>(),
                vec![8, 4, 4, 4, 12],
                "case {case}: expected 8-4-4-4-12 groups, got {id:?}"
            );
            assert!(
                groups
                    .iter()
                    .all(|group| group.bytes().all(|b| b.is_ascii_hexdigit()
                        && !b.is_ascii_uppercase())),
                "case {case}: expected lowercase hex, got {id:?}"
            );
            assert!(
                !seen.contains(&id),
                "case {case}: expected a fresh id, got the already-minted {id:?}"
            );
            seen.push(id);
        }
    }
}

