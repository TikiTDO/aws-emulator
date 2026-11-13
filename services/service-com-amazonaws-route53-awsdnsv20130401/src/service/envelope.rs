// SPDX-License-Identifier: CC0-1.0

//! Route 53's error envelope: the one place every failure this service answers is
//! finished off before it goes on the wire.
//!
//! Route 53 is `restXml`, and its error document is
//! `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId/></ErrorResponse>`:
//! the `Error` element states WHAT failed, and the `RequestId` BESIDE it names the
//! request it failed for, so a caller — or the SDK middleware that reads it for retry
//! correlation and support escalation — can tie the failure to the service's own record
//! of it. `RequestId` is a property of the ENVELOPE, not a member of any error shape the
//! model declares (`smithy_query(query="shape",shape="NoSuchHostedZone")` lists only
//! `message`), so the protocol serializer — which renders an error from its shape's
//! members — cannot put it there. [`ErrorEnvelope`] does, for every response this
//! service answers: a modelled error a handler raised, and a refusal the request
//! boundary made before any handler ran, alike.
//!
//! Where the element BELONGS is the vendored wrapped-restXml renderer
//! (`aws_smithy_schema_server::rest_xml`'s `serialize_error_wrapped`, which already
//! writes a `RequestId` on the `noErrorWrapping` path beside the `x-amz-request-id`
//! header mirroring it). That tree is read-only from here, and the framework's
//! pre-dispatch renderer for a boundary refusal lives in it too, so the element is
//! added to the finished document instead — delete both halves of this seam once the
//! vendored renderer carries it. Three sibling services splice the same element into
//! their own envelope the same way —
//! `service-com-amazonaws-cloudfront-cloudfront2020-05-31`'s `service::envelope` (also
//! `restXml`), `service-com-amazonaws-cloudformation-cloudformation`'s
//! `query_error_envelope` and
//! `service-com-amazonaws-sns-amazonsimplenotificationservice`'s `service` (both
//! awsQuery) — so the four are named here together: the splice is per-service today
//! because the envelope and the renderer that writes it are, and hoisting all four
//! belongs with the vendored change that makes them unnecessary rather than with this
//! fix.
//!
//! EVERY failure this service answers passes through here: a modelled error and a
//! boundary refusal reach [`ErrorEnvelope`] on the dispatch path, and the three the
//! router renders OUTSIDE it — a request that names no modelled operation, a
//! transport-level failure, and a refusal the router makes before any operation runs —
//! reach [`Route53Definition`], which wraps the generated definition's own renderers.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationRoutingTable,
    ServerHttpProtocol, Service, ServiceIdentity,
};

/// The envelope element the request-correlation id sits beside `Error` in.
const REQUEST_ID_ELEMENT: &str = "RequestId";

/// The envelope root, whose close is where a missing [`REQUEST_ID_ELEMENT`] is added.
const ERROR_RESPONSE_OPEN: &str = "<ErrorResponse";
const ERROR_RESPONSE_CLOSE: &str = "</ErrorResponse>";

/// The body of `resp` as text, when it is in memory and valid UTF-8.
fn body_text(resp: &RtResponse) -> Option<&str> {
    std::str::from_utf8(resp.body().bytes()?).ok()
}

/// The body `resp` should carry so its error envelope names the request, or `None` when
/// it is not an envelope that is missing one (a success, an empty body, an envelope that
/// already carries the element).
fn request_id_insertion(resp: &RtResponse) -> Option<String> {
    // A success is never an error envelope, whatever its document happens to contain:
    // the status is what says this response states a failure.
    if resp.status().as_u16() < 400 {
        return None;
    }
    let body = body_text(resp)?;
    if !body.contains(ERROR_RESPONSE_OPEN) || body.contains(&format!("<{REQUEST_ID_ELEMENT}>")) {
        return None;
    }
    let close_at = body.rfind(ERROR_RESPONSE_CLOSE)?;
    let mut rewritten = String::with_capacity(body.len() + 64);
    rewritten.push_str(&body[..close_at]);
    rewritten.push_str(&format!(
        "<{REQUEST_ID_ELEMENT}>{}</{REQUEST_ID_ELEMENT}>",
        crate::state::new_request_id()
    ));
    rewritten.push_str(&body[close_at..]);
    Some(rewritten)
}

/// `resp` with its error envelope naming the request it answers.
///
/// A response that is not a restXml error envelope, or one that already names the
/// request, is handed back untouched — so this is applied to EVERY response without
/// having to know which ones are errors.
pub fn with_request_id(mut resp: RtResponse) -> RtResponse {
    let Some(body) = request_id_insertion(&resp) else {
        return resp;
    };
    let length = body.len();
    *resp.body_mut() = SdkBody::from(body);
    // Only a response that already states its length gets the new one: a renderer that
    // left the framing to the transport keeps doing so.
    if resp.headers().get("content-length").is_some() {
        resp.headers_mut()
            .insert("content-length", length.to_string());
    }
    resp
}

/// The generated definition, with the error envelope finished on the three failures the
/// router renders OUTSIDE the dispatch path: a request that names no modelled operation,
/// a transport-level failure, and a refusal made before any operation runs. Each is the
/// generated renderer's own document with the envelope's request id added, so a caller
/// correlating a failure never has to care which layer refused it.
///
/// Identity, the protocol and the routing table come from the generated
/// [`crate::ServiceDefinition`] untouched — nothing model-derived is restated here.
pub struct Route53Definition<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> Route53Definition<Eng> {
    /// Build the definition from the shared engine, which the generated entry threads
    /// into every operation.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }

    /// Whether the generated routing table can serve this request — the condition under
    /// which this service claims it for [`ErrorEnvelope`].
    pub fn routes(&self, req: &RtRequest) -> bool {
        self.entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.entry.routing_table().get(key))
            .is_some()
    }
}

impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for Route53Definition<Eng> {
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
        with_request_id(self.entry.routing_error_response(req))
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        with_request_id(self.entry.framework_error_response(error, req))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        with_request_id(ErasedServiceDefinition::refusal_response(
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

/// The modelled operation the request names, read from the model-derived routing table
/// (never from the service's own override, which routes straight back here).
fn modelled_operation<'s>(
    svc: &'s dyn Service,
    req: &RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// The seam every request this service serves runs through: it dispatches the modelled
/// operation unchanged, then finishes the response's error envelope (see the module
/// docs).
///
/// Stateless and engine-free: the operation it runs is looked up on the service the
/// router hands it, so this holds nothing per request and nothing per engine.
#[derive(Debug, Default)]
pub struct ErrorEnvelope;

impl ErasedOperation for ErrorEnvelope {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The generated deserialize -> validate -> handle -> serialize walk, run
            // exactly as the router would have run it. A request that names no modelled
            // operation never reaches here: the service claims only the ones its routing
            // table resolves.
            let Some(op) = modelled_operation(svc, req) else {
                return with_request_id(svc.definition().routing_error_response(req));
            };
            // The request SCHEMA is checked before the request is read, exactly as
            // the service checks it while parsing the body: an element whose
            // accepted values the schema enumerates is held to that set here, so no
            // handler is ever given a value the service would not have parsed. The
            // renderer it answers through already finishes the envelope.
            if let Some(refused) = crate::service::xml_schema::refusal(svc, req) {
                return refused;
            }
            // The resource type a tag request names is put in scope around the run, so
            // a refusal of one of its resource ids can be stated at the field that type
            // reports its own id at: the seam that words a refusal
            // (`Service::override_refusal`) is handed no request, and the type is a
            // property of this one. Every other request is run untouched.
            with_request_id(
                crate::service::tag_resource_id::serving(req, op.run(svc, req, ctx)).await,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    //! The envelope surgery itself, read off documents rather than through the router
    //! (the crate's integration suite drives the same rule end to end over HTTP).

    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;

    /// One response with `status` and `body`.
    fn response(status: u16, body: &str) -> RtResponse {
        RtResponse::new(
            StatusCode::try_from(status).expect("the test status must be valid"),
            SdkBody::from(body.to_owned()),
        )
    }

    /// The text of the first `<RequestId>` element of a document.
    fn request_id(body: &str) -> Option<&str> {
        let after_open = body.split_once("<RequestId>")?.1;
        Some(after_open.split_once("</RequestId>")?.0)
    }

    /// An error envelope with no `RequestId` gets one, as the LAST child of the
    /// envelope root (after `</Error>`), whatever status the failure carries.
    #[test]
    fn an_envelope_without_a_request_id_gets_one() {
        let body = "<?xml version=\"1.0\"?><ErrorResponse xmlns=\"ns\"><Error><Type>Sender</Type>\
             <Code>NoSuchHostedZone</Code><Message>m</Message></Error></ErrorResponse>";
        for status in [400u16, 404, 409, 500] {
            let answered = with_request_id(response(status, body));
            let answered = body_text(&answered)
                .unwrap_or_else(|| panic!("case {status}: the body must still be readable"))
                .to_string();
            let id = request_id(&answered);
            assert!(
                id.is_some_and(|id| !id.is_empty()),
                "case {status}: expected a RequestId element, got {answered:?}"
            );
            assert!(
                answered.contains(&format!(
                    "</Error><RequestId>{}</RequestId></ErrorResponse>",
                    id.unwrap_or_default()
                )),
                "case {status}: expected RequestId as the envelope's last child, got \
                 {answered:?}"
            );
        }
    }

    /// A document that is not an error envelope this service is missing an id on is
    /// handed back byte for byte: a success (whatever it contains), an envelope that
    /// already names the request, and a failure rendered in some other document.
    #[test]
    fn a_response_that_needs_no_request_id_is_unchanged() {
        let cases = [
            (
                "a success",
                200u16,
                "<GetHostedZoneResponse><HostedZone/></GetHostedZoneResponse>",
            ),
            (
                "an envelope that already names the request",
                404,
                "<ErrorResponse><Error/><RequestId>already</RequestId></ErrorResponse>",
            ),
            ("a failure in another document", 500, "<Error><Code>x</Code></Error>"),
            ("an empty body", 400, ""),
        ];
        for (case, status, body) in cases {
            let answered = with_request_id(response(status, body));
            assert_eq!(
                body_text(&answered),
                Some(body),
                "case {case:?}: the document must be handed back unchanged"
            );
        }
    }

    /// Two responses carry two different ids, so a caller can tell the failures apart.
    #[test]
    fn two_responses_name_two_requests() {
        let envelope = "<ErrorResponse><Error/></ErrorResponse>";
        let first = with_request_id(response(404, envelope));
        let second = with_request_id(response(404, envelope));
        let first = body_text(&first).map(str::to_string).unwrap_or_default();
        let second = body_text(&second).map(str::to_string).unwrap_or_default();
        assert_ne!(
            request_id(&first),
            request_id(&second),
            "two responses must name two requests, got {first:?} and {second:?}"
        );
    }
}
