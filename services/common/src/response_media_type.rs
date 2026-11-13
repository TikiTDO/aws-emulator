// SPDX-License-Identifier: CC0-1.0

//! The media type a service ANNOUNCES on its responses, where that is not the one its
//! protocol trait implies.
//!
//! # The rule
//!
//! A response's `content-type` is the media type the SERVICE announces, which is a fact of
//! its recorded wire rather than a consequence of the protocol trait its model carries: a
//! service whose model declares `aws.protocols#awsJson1_0` and whose responses are recorded
//! as `application/x-amz-json-1.1` answers `1.1` — on every status, success and error
//! alike — because that is what a client reads. The framework's renderers stamp the media
//! type the protocol trait implies (`AwsJson10()` in
//! `vendor/smithy-schema-server/src/rpc.rs` for a serialized body, the service macro's
//! per-protocol pre-dispatch declaration for a framework-rendered one), so a service whose
//! wire differs from it re-announces that ONE value and leaves every other response header
//! alone.
//!
//! A service's wire can show MORE than one announcer, and then which layer answered decides
//! the value: a front door that announces its own media type on what its handlers produce,
//! with another layer in front of it — a request validator, say — recorded under a
//! different one. That is still this rule, applied per announcer: the service states which
//! responses one announcer produced ([`AnnouncedResponses`], reading the response a client
//! receives, because which internal seam rendered it is the emulator's business and not the
//! service's wire) and the rest keep the media type their own producer stamped. Which
//! announcer owns which responses is a fact of the recorded wire, never a guess. The one
//! such split that is not a single service's question lives here too: a refusal reporting
//! the REQUEST as invalid is the validator's and everything else — a success, an error about
//! stored state, and a refusal of a WELL-FORMED request on the CALLER's account class — is
//! the front door's ([`not_an_input_validation_refusal`]). What decides it is the GROUND the
//! refusal states, not the error shape it is answered in: a service can answer both grounds
//! in one modelled shape, and then the shape name tells the two announcers apart nowhere.
//!
//! The REQUEST side is untouched: the media type a caller sends is not re-read, routing
//! still follows the model's protocol, and the awsJson `1.0` and `1.1` codecs are the same
//! `JsonCodec` over the same body, so only the announced value differs.
//!
//! # Where a service takes it
//!
//! [`ResponseMediaType::announce`] is the whole mechanism; which seams a service applies it
//! at is the service's own business, because the layers that can produce a response differ
//! per service (`vendor/smithy-server-runtime/src/service.rs` has their contracts):
//!
//! * `Service::override_protocol` can serve the codec that already announces the right
//!   value — for the awsJson family, `aws_smithy_schema_server::rpc::AwsJson11()` — which
//!   covers every success and every error a handler raises;
//! * the framework-rendered responses — the refusal tail inside the centralized walk, the
//!   routing failure, a transport failure — keep the model-derived envelope, which
//!   `override_protocol` deliberately does not reach, and are re-announced with
//!   [`ResponseMediaType::announce`] through `Service::override_operation`,
//!   `Service::override_routing_error` and `Service::override_framework_error`.
//!
//! The first of those three is the one that needs plumbing rather than a one-line call: the
//! refusal tail is rendered INSIDE the centralized walk, so reaching it means serving that
//! walk and re-announcing what it answered. That plumbing is therefore stated here once, as
//! [`ResponseMediaTypeOperation`] (the wrapper a service hands back from
//! `Service::override_operation`, re-announcing either every response of the walk or only
//! the ones an [`AnnouncedResponses`] predicate accepts) and [`routes`] (the condition under
//! which it claims a request); the other two seams are handed the rendered response and need
//! nothing but the call.
//!
//! One response no author seam reaches: the authentication refusal, which the router renders
//! through `definition().refusal_response(..)` before the service is asked anything
//! (`vendor/smithy-server-runtime/src/multi_service_router.rs`), so it keeps the
//! protocol-implied value.
//!
//! Nothing else about a response is touched: the status, every other header and every body
//! byte stay with the renderer that owns them, and a response announcing a media type that
//! is NOT the protocol-implied one — an event-stream frame, say — is left exactly as its
//! producer wrote it rather than relabelled.
//!
//! This rule is CALLED by
//! `service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator`,
//! `service-com-amazonaws-freetier-awsfreetierservice` and
//! `service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice` — the same AWS billing
//! front door, the same `1.1` framing over an `awsJson1_0` model; the last of them is the
//! two-announcer case, so it states which responses that front door produced rather than
//! re-announcing all of them — and by
//! `service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice` and
//! `service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2`, whose models declare
//! `restJson1` and whose recorded wires announce the awsJson `1.1` media type on every
//! reply, so each states its own pair (`application/json` → `1.1`) and takes the
//! mechanism, including [`ResponseMediaTypeOperation`], from here. Each of them states
//! only its own pair (`RESPONSE_MEDIA_TYPE`).
//!
//! STILL TO MIGRATE onto this module: `service-com-amazonaws-amp-amazonprometheusservice`
//! (`RESPONSE_CONTENT_TYPE` in its `src/state.rs`, with the rewrite in its
//! `src/service.rs`) and
//! `service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions`
//! (which takes `override_protocol` alone, so the responses its framework renders still
//! announce the protocol-implied value). Each is a change of its own rather than a side
//! effect of the fix that hoisted the rule — both need a path dependency their crate does
//! not carry yet, and neither's issues or recordings is owned here — but nothing new
//! should state the rule again.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Service,
};

/// The response header carrying the media type.
const CONTENT_TYPE: &str = "content-type";

/// The media type the `aws.protocols#awsJson1_0` protocol trait implies.
pub const AWS_JSON_1_0: &str = "application/x-amz-json-1.0";

/// The media type the `aws.protocols#awsJson1_1` protocol trait implies.
pub const AWS_JSON_1_1: &str = "application/x-amz-json-1.1";

/// The pair one service's responses are re-announced across: the media type its protocol
/// trait implies, and the one its wire record shows it answers under.
///
/// Held as a value (and constructible in a `const`) so a service states the pair once and
/// every seam it re-announces at reads the same two strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseMediaType {
    implied: &'static str,
    announced: &'static str,
}

impl ResponseMediaType {
    /// The pair `implied` → `announced`: a response carrying `implied` is re-announced as
    /// `announced`, and nothing else is touched.
    pub const fn new(implied: &'static str, announced: &'static str) -> Self {
        Self { implied, announced }
    }

    /// The media type the service's protocol trait implies — the only value
    /// [`Self::announce`] replaces.
    pub const fn implied(&self) -> &'static str {
        self.implied
    }

    /// The media type the service announces on every response.
    pub const fn announced(&self) -> &'static str {
        self.announced
    }

    /// `resp` as the service announces it: the protocol-implied media type replaced by the
    /// announced one. A response carrying any other media type, or none at all, is returned
    /// untouched — see the module header.
    ///
    /// The comparison is on the WHOLE header value, not on the media type with its
    /// parameters stripped, because the values this replaces are constants: the framework's
    /// renderers stamp the bare protocol media type (`AwsJson10()` and the service macro's
    /// pre-dispatch declaration both carry no parameter). A `content-type` that carries a
    /// parameter was therefore chosen by its producer for that response, and rewriting it
    /// would drop what the producer said — so it is left alone, like any other value.
    pub fn announce(&self, mut resp: RtResponse) -> RtResponse {
        if resp.headers().get(CONTENT_TYPE) == Some(self.implied) {
            resp.headers_mut().insert(CONTENT_TYPE, self.announced);
        }
        resp
    }
}

/// Whether `definition`'s generated routing table can serve this request — the condition
/// under which a service claims it for [`ResponseMediaTypeOperation`].
///
/// A request the table cannot serve is declined rather than claimed, so the router answers
/// it as the unroutable request it is (through `Service::override_routing_error`) instead of
/// authenticating a request that names no operation first.
pub fn routes(definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> bool {
    definition
        .protocol()
        .routing_key(req)
        .ok()
        .and_then(|key| definition.routing_table().get(key))
        .is_some()
}

/// Serves the modelled operation the service's routing table names for the request, exactly
/// as that table would, then announces the response the way the service announces it.
///
/// What it is FOR depends on the other seams its service takes. A service that also serves
/// a codec through `Service::override_protocol` is already announcing every response that
/// codec serialized, and takes this wrapper for the one response that seam explicitly does
/// not reach: the framework-level refusal tail, which renders a constraint the model
/// declares in the model-derived envelope INSIDE the centralized walk
/// (`run_operation_http`). A service that keeps the model's protocol takes it for the whole
/// walk instead, and says with an [`AnnouncedResponses`] predicate which of that walk's
/// responses its announcer produced.
///
/// Either way it reaches only a ROUTED request: what the router renders outside the walk —
/// an unroutable request, a transport-level failure, the authentication refusal — never
/// passes through here and keeps the media type its own renderer stamped.
///
/// It holds no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`] — a wrapper held on the service cannot borrow from the entry it
/// is held beside, which is why the key is resolved here rather than carried in. So the
/// framework keeps the whole deserialize → validate → handle → serialize walk and every
/// status, code, message and body byte it answers with, and this wrapper owns one header.
#[derive(Debug, Clone, Copy)]
pub struct ResponseMediaTypeOperation {
    media_type: ResponseMediaType,
    announced: Option<AnnouncedResponses>,
}

/// Which of a walk's responses one announcer produced, for a service whose wire shows more
/// than one of them.
///
/// A service answering under a single media type re-announces every response and states no
/// predicate. Where two layers answer under different media types, the predicate is how the
/// service says which responses belong to the layer it is re-announcing for; the rest are
/// handed back as their producer wrote them.
pub type AnnouncedResponses = fn(&RtResponse) -> bool;

/// The error code of the refusal a request validator answers with, which the awsJson
/// envelope carries as `__type`: the modelled `ValidationException` every emulated service
/// on a JSON protocol declares for an input that does not satisfy its requirements.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `ValidationExceptionReason`'s value naming the CALLER — an account whose bills another
/// account in its organization pays — rather than anything in the request, as the ground a
/// well-formed request was refused on: `invalidRequestFromMember`, as the modelled enum
/// spells it.
///
/// A refusal on that ground is answered by the layer that decides what the CALLER may ask
/// for, which sits behind the request validator, so it is not the validator's response even
/// though both are worded in the same modelled shape. A service's gate may put the value on
/// the wire in its own spelling — the recorded wire of an AWS billing front door carries the
/// SCREAMING_SNAKE_CASE rendering of the enum value — so the two spellings are folded
/// together ([`names_the_callers_account_class`]).
const REASON_INVALID_REQUEST_FROM_MEMBER: &str = "invalidRequestFromMember";

/// The awsJson error envelope `resp` carries, when `resp` is a client error whose body is
/// one — read off the response the CLIENT receives, not off the seam that rendered it.
fn client_error_envelope(resp: &RtResponse) -> Option<serde_json::Value> {
    if !resp.status().is_client_error() {
        return None;
    }
    serde_json::from_slice(resp.body().bytes()?).ok()
}

/// The shape an awsJson error envelope names in `__type`: the shape name alone, since
/// `__type` may qualify it by its namespace or by the `<version>#` prefix the awsJson
/// protocols allow.
fn envelope_shape(envelope: &serde_json::Value) -> Option<&str> {
    envelope
        .get("__type")
        .and_then(serde_json::Value::as_str)
        .and_then(|code| code.split(['#', '/']).next_back())
}

/// Whether `reason` — the `ValidationExceptionReason` a refusal reports — is the value naming
/// the caller's account class ([`REASON_INVALID_REQUEST_FROM_MEMBER`]), in either spelling a
/// service puts an enum value on the wire in: the model's own lowerCamel, or the
/// SCREAMING_SNAKE_CASE rendering of it. Folding the `_` separators and the case away is what
/// makes the two the same value rather than two.
fn names_the_callers_account_class(reason: &str) -> bool {
    fn folded(value: &str) -> String {
        value
            .chars()
            .filter(|character| *character != '_')
            .flat_map(char::to_lowercase)
            .collect()
    }
    folded(reason) == folded(REASON_INVALID_REQUEST_FROM_MEMBER)
}

/// Whether `resp` is the refusal a REQUEST VALIDATOR answers: a client error whose awsJson
/// envelope names the modelled `ValidationException` in `__type` AND whose stated ground is
/// the request itself rather than the caller's account class.
///
/// The ground is read from the modelled `reason` member, the one half of the body that states
/// it: a refusal naming the caller's account class
/// ([`names_the_callers_account_class`]) is the account-class layer's, and every other
/// `ValidationException` — one reporting a member of the request at fault, with or without a
/// `reason` — is the validator's.
///
/// Read off the response rather than off the seam that rendered it, because which seam
/// refuses a constraint is the emulator's business: a boundary validator refuses what the
/// model's constraints express and a handler refuses what they cannot, and both are the one
/// refusal to the client.
#[must_use]
pub fn is_input_validation_refusal(resp: &RtResponse) -> bool {
    let Some(envelope) = client_error_envelope(resp) else {
        return false;
    };
    if envelope_shape(&envelope) != Some(VALIDATION_EXCEPTION) {
        return false;
    }
    envelope
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|reason| !names_the_callers_account_class(reason))
}

/// The [`AnnouncedResponses`] predicate for a service whose front door announces its own
/// media type on what it answers while the REQUEST VALIDATOR in front of it answers under
/// the protocol-implied one: every response but the input-validation refusal is the front
/// door's ([`is_input_validation_refusal`]).
///
/// Stated here because it is not one service's question — a success, an error about stored
/// state, a body the codec cannot read and a refusal of a well-formed request on the
/// caller's account class are the front door's, a refusal reporting the request as invalid
/// is the validator's — so two services on that wire cannot disagree about which announcer
/// answered.
#[must_use]
pub fn not_an_input_validation_refusal(resp: &RtResponse) -> bool {
    !is_input_validation_refusal(resp)
}

impl ResponseMediaTypeOperation {
    /// The wrapper that re-announces EVERY response of the walk across `media_type`. A
    /// service builds one in its constructor, because `Service::override_operation` hands
    /// back a borrow of it.
    pub const fn new(media_type: ResponseMediaType) -> Self {
        Self {
            media_type,
            announced: None,
        }
    }

    /// The same wrapper, re-announcing only the responses `announced` accepts — the ones
    /// the announcer this pair belongs to produced. Every other response keeps the media
    /// type its own producer stamped.
    pub const fn announcing(
        media_type: ResponseMediaType,
        announced: AnnouncedResponses,
    ) -> Self {
        Self {
            media_type,
            announced: Some(announced),
        }
    }

    /// `resp` as this wrapper announces it: re-announced across the pair when it is one of
    /// the responses this announcer produced, and handed back exactly as its producer wrote
    /// it otherwise. A wrapper built by [`Self::new`] announces every response.
    pub fn announce(&self, resp: RtResponse) -> RtResponse {
        if self.announced.is_none_or(|produced_here| produced_here(&resp)) {
            self.media_type.announce(resp)
        } else {
            resp
        }
    }
}

impl ErasedOperation for ResponseMediaTypeOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let definition = svc.definition();
            let modeled = definition
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| definition.routing_table().get(key));
            let resp = match modeled {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable through a `Service::override_operation` that claims a request
                // only when the routing table holds its key ([`routes`]); answered as the
                // framework answers an unroutable request rather than guessed at.
                None => definition.routing_error_response(req),
            };
            self.announce(resp)
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is the
        // operation this wrapper delegates to, so its model-declared schemes decide and this
        // wrapper cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

    /// A response carrying `content_type`, or none at all when it is `None`.
    fn response(content_type: Option<&str>) -> RtResponse {
        let mut resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200).expect("200 is a status"),
            SdkBody::from(&b"{}"[..]),
        );
        if let Some(content_type) = content_type {
            resp.headers_mut()
                .insert(CONTENT_TYPE, content_type.to_string());
        }
        resp
    }

    /// The media type of a response, as a client would read it.
    fn media_type(resp: &RtResponse) -> Option<String> {
        resp.headers().get(CONTENT_TYPE).map(str::to_string)
    }

    /// The protocol-implied media type becomes the announced one; every other value, and a
    /// response with no media type at all, is left alone.
    ///
    /// The rows span what a producer can put in the header — the implied value (what the
    /// framework's renderers stamp), the announced value (already right), a deliberately
    /// different media type (an event-stream frame, a REST JSON body), and no header — and
    /// the pairs span both directions, so the replacement is the stated one rather than
    /// "whatever looks like awsJson".
    #[test]
    fn only_the_implied_media_type_is_reannounced() {
        let pairs = [
            ResponseMediaType::new(AWS_JSON_1_0, AWS_JSON_1_1),
            ResponseMediaType::new(AWS_JSON_1_1, AWS_JSON_1_0),
            ResponseMediaType::new("application/json", "application/json; charset=utf-8"),
        ];
        let others = [
            "application/vnd.amazon.eventstream",
            "text/xml",
            "application/cbor",
        ];

        for pair in pairs {
            let actual = media_type(&pair.announce(response(Some(pair.implied()))));
            assert_eq!(
                actual.as_deref(),
                Some(pair.announced()),
                "case {pair:?} on its implied media type: expected {:?}, got {actual:?}",
                pair.announced()
            );

            let actual = media_type(&pair.announce(response(Some(pair.announced()))));
            assert_eq!(
                actual.as_deref(),
                Some(pair.announced()),
                "case {pair:?} on its announced media type: expected it unchanged, got \
                 {actual:?}"
            );

            for other in others {
                let actual = media_type(&pair.announce(response(Some(other))));
                assert_eq!(
                    actual.as_deref(),
                    Some(other),
                    "case {pair:?} on {other:?}: a media type the producer chose must be \
                     left alone, got {actual:?}"
                );
            }

            let actual = media_type(&pair.announce(response(None)));
            assert_eq!(
                actual, None,
                "case {pair:?} on a response with no media type: expected none, got \
                 {actual:?}"
            );
        }
    }

    /// A wrapper re-announces every response of the walk, or only the ones its predicate
    /// says its announcer produced — and the predicate decides from the response, so two
    /// responses of one walk can come out announced differently.
    ///
    /// The rows span the three forms a service states: no predicate (every response), a
    /// predicate accepting a response (re-announced) and one refusing it (left as its
    /// producer wrote it); `status_is_ok` is a stand-in for a real service's "which layer
    /// produced this" question.
    #[test]
    fn a_predicate_decides_which_responses_are_reannounced() {
        /// Accepts the responses of a successful call, refuses a refusal.
        fn status_is_ok(resp: &RtResponse) -> bool {
            resp.status().is_success()
        }

        let pair = ResponseMediaType::new(AWS_JSON_1_0, AWS_JSON_1_1);
        let every = ResponseMediaTypeOperation::new(pair);
        let only_successes = ResponseMediaTypeOperation::announcing(pair, status_is_ok);

        let cases: [(&str, ResponseMediaTypeOperation, u16, &str); 4] = [
            ("no predicate, on a success", every, 200, AWS_JSON_1_1),
            ("no predicate, on a refusal", every, 400, AWS_JSON_1_1),
            (
                "a predicate that accepts it",
                only_successes,
                200,
                AWS_JSON_1_1,
            ),
            (
                "a predicate that refuses it",
                only_successes,
                400,
                AWS_JSON_1_0,
            ),
        ];

        for (case, wrapper, status, expected) in cases {
            let mut resp = RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(status)
                    .expect("a status code"),
                SdkBody::from(&b"{}"[..]),
            );
            resp.headers_mut().insert(CONTENT_TYPE, AWS_JSON_1_0);
            let actual = media_type(&wrapper.announce(resp));
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {case:?} (HTTP {status}): expected content-type {expected}, got \
                 {actual:?}"
            );
        }
    }

    /// A refusal reporting the REQUEST as invalid is the request validator's; every other
    /// response — including a refusal of a well-formed request on the CALLER's account class,
    /// worded in the same modelled shape — is the front door's.
    ///
    /// The rows span the envelopes a client can receive: the input-validation refusal
    /// qualified and unqualified, the same refusal stating its ground in `reason`, the
    /// account-class refusal in both spellings a service puts that enum value on the wire in,
    /// another client error of the same status, a client error the framework renders before
    /// the operation, a success, a server error, and a body that is not the JSON envelope at
    /// all.
    #[test]
    fn only_a_refusal_about_the_request_is_the_validators() {
        /// A response carrying `body` under `status`, as a layer of the walk renders one.
        fn answered(status: u16, body: &str) -> RtResponse {
            RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status"),
                SdkBody::from(body.as_bytes().to_vec()),
            )
        }

        let cases: [(&str, RtResponse, bool); 10] = [
            (
                "the input-validation refusal",
                answered(
                    400,
                    r#"{"__type":"ValidationException","message":"1 validation error detected"}"#,
                ),
                true,
            ),
            (
                "the input-validation refusal, namespace-qualified",
                answered(
                    400,
                    r#"{"__type":"com.amazonaws.example#ValidationException","message":"x"}"#,
                ),
                true,
            ),
            (
                "the input-validation refusal stating its own ground",
                answered(
                    400,
                    r#"{"__type":"ValidationException","reason":"fieldValidationFailed","message":"x"}"#,
                ),
                true,
            ),
            (
                "the account-class refusal, the gate's spelling of the reason",
                answered(
                    400,
                    r#"{"__type":"ValidationException","reason":"INVALID_REQUEST_FROM_MEMBER","message":"x"}"#,
                ),
                false,
            ),
            (
                "the account-class refusal, the model's spelling of the reason",
                answered(
                    400,
                    r#"{"__type":"ValidationException","reason":"invalidRequestFromMember","message":"x"}"#,
                ),
                false,
            ),
            (
                "a refusal about stored state, same status",
                answered(400, r#"{"__type":"ResourceNotFoundException","message":"x"}"#),
                false,
            ),
            (
                "a client error the framework renders before the operation",
                answered(400, r#"{"__type":"SerializationException","message":"x"}"#),
                false,
            ),
            ("a success", answered(200, r#"{"items":[]}"#), false),
            (
                "a server error",
                answered(500, r#"{"__type":"InternalServerException","message":"x"}"#),
                false,
            ),
            (
                "a body that is not the JSON envelope",
                answered(400, "not json"),
                false,
            ),
        ];

        for (case, resp, expected) in cases {
            let actual = is_input_validation_refusal(&resp);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected is_input_validation_refusal {expected}, got {actual}"
            );
            assert_eq!(
                not_an_input_validation_refusal(&resp),
                !expected,
                "case {case:?}: the announcer predicate must be the inverse of the refusal \
                 question"
            );
        }
    }
}
