// SPDX-License-Identifier: CC0-1.0

//! The awsQuery error envelope this service answers with: `<ErrorResponse>` carrying a
//! `<RequestId>` sibling of `<Error>`.
//!
//! AWS answers a failed awsQuery call with
//!
//! ```xml
//! <ErrorResponse xmlns="http://elasticache.amazonaws.com/doc/2015-02-02/">
//!   <Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error>
//!   <RequestId>…</RequestId>
//! </ErrorResponse>
//! ```
//!
//! The `<RequestId>` element belongs to the RESPONSE, not to any modelled error shape:
//! it sits OUTSIDE `<Error>`, so no error shape's members can carry it and the
//! framework's query error serializer — which writes the error shape's own members
//! inside `<Error>` — does not either. It is what an SDK's error object exposes as the
//! request id and what correlates a failed call against server-side records, so a
//! failure must carry one exactly as a success carries `ResponseMetadata/RequestId`.
//!
//! This module therefore adds the element to EVERY error response this service emits,
//! whichever layer produced it:
//!
//! * a handler-returned or modelled error, the scaffold's 501, and a framework refusal
//!   raised while a matched operation was being served — an absent required member, a
//!   value outside a declared bound or outside an enum's declared set — through
//!   [`RequestIdEnvelopeOperation`], the operation wrapper this service hands back from
//!   `Service::override_operation`;
//! * a routing failure (an `Action` naming no modelled operation), a transport failure
//!   and a pre-dispatch refusal, through [`ElastiCacheDefinition`], the definition this
//!   service hands back from `Service::definition`.
//!
//! Only the envelope is this module's business: the HTTP status, the `<Type>`, the
//! `<Code>`, the `<Message>` and the whole success path stay with the framework renderer
//! that owns them, so each has exactly one home.
//!
//! # Why the rule is stated here
//!
//! The rule is the PROTOCOL's, not this service's, and its real home is the framework's
//! query error renderer, which this emulator cannot edit (`vendor/` is read-only). The
//! next best home would be `services/common`, and a previous change did write it there —
//! but `services/common/src/query_error_envelope.rs` is an emptied, undeclared tombstone
//! on mainline, whose own text records that the services carrying their own writer
//! (`autoscaling`, `cloudformation`, `elasticbeanstalk`, `rds` and `sns`) consolidate
//! under THEIR issues, not under a fix for one more service. This module follows that
//! decision: the writer lives beside the wiring that uses it, matching
//! `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/service/query_error_envelope.rs`
//! element for element, so the consolidation that eventually lands has one shape to
//! lift rather than six variants to reconcile.
//!
//! The one deliberate difference from those copies is where the id comes from: it is
//! built from `sha2` and `hex`, which this crate already depends on, rather than from a
//! new `uuid` dependency — see [`new_request_id`] for why this change cannot afford an
//! edit to `Cargo.lock`.

use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use sha2::{Digest, Sha256};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationRoutingTable,
    ServerHttpProtocol, Service, ServiceIdentity,
};

/// The closing tag of the awsQuery error envelope, and the only marker that tells an
/// error document apart from a success envelope (whose own request id already sits in
/// `<ResponseMetadata>`).
const ERROR_RESPONSE_CLOSE: &str = "</ErrorResponse>";

/// The closing tag of the element the request id is a SIBLING of: what sits between it
/// and [`ERROR_RESPONSE_CLOSE`] is the only place a `<RequestId>` of this document can be.
const ERROR_ELEMENT_CLOSE: &str = "</Error>";

/// The envelope element this module adds.
const REQUEST_ID_OPEN: &str = "<RequestId>";

/// The header AWS reports the same id in, so a client correlating on the header and one
/// reading the document see one value.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The value every id of this process is derived from: a per-process seed, taken from the
/// standard library's randomly seeded `RandomState` and the instant the first id was
/// asked for, so two processes do not walk the same sequence.
static SEED: LazyLock<[u8; 16]> = LazyLock::new(|| {
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or_default();
    let mut seed = [0u8; 16];
    seed[..8].copy_from_slice(&random.to_be_bytes());
    seed[8..].copy_from_slice(&since_epoch.to_be_bytes());
    seed
});

/// What makes two ids of one process distinct, however close together they are minted.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Mint the id one response reports as its own.
///
/// A correlation id, not a security token — the same thing the framework's query renderer
/// puts in a successful response's `ResponseMetadata/RequestId` — so it only has to be
/// distinct per response and shaped the way AWS answers: the 8-4-4-4-12 hexadecimal form,
/// with RFC 4122's version-4 and variant bits, built here from SHA-256 over the
/// per-process [`SEED`] and a monotonic counter (`sha2` and `hex`, which this crate
/// already depends on for its ARNs and pagination markers). Deliberately NOT a new
/// `uuid` dependency: that would put this crate's whole change behind an edit to
/// `Cargo.lock`, the one file every concurrent fix also edits, and that collision is what
/// kept this change's squash refusing to land — the same reasoning the wire harness in
/// `tests/all/cache_cluster_lifecycle.rs` records for not adding an XML crate. It needs
/// no state of this service, so a request that failed before the store was reached still
/// answers with one.
pub fn new_request_id() -> String {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut digest = Sha256::new();
    digest.update(*SEED);
    digest.update(sequence.to_be_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    // The version (4) and variant (RFC 4122) nibbles an AWS request id carries.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Add `<RequestId>` to an awsQuery `<ErrorResponse>` document, as the last child of the
/// envelope (the sibling after `<Error>`).
///
/// `None` means the document needs nothing: it is not an `<ErrorResponse>` envelope (a
/// successful `<…Response>` carries its request id in `ResponseMetadata` already), or it
/// already carries one in the SIBLING position — so this is idempotent, and a framework
/// renderer that starts emitting the element itself cannot end up with two.
fn add_request_id(body: &str, request_id: &str) -> Option<String> {
    // The LAST occurrence: every interpolated value in the document is XML-escaped by the
    // renderer, so a closing tag cannot appear inside one (the unit tests below pin that
    // invariant on a message that quotes both tags).
    let close = body.rfind(ERROR_RESPONSE_CLOSE)?;
    let siblings = match body[..close].rfind(ERROR_ELEMENT_CLOSE) {
        Some(error_end) => &body[error_end..close],
        None => &body[..close],
    };
    if siblings.contains(REQUEST_ID_OPEN) {
        return None;
    }
    Some(format!(
        "{}<RequestId>{request_id}</RequestId>{}",
        &body[..close],
        &body[close..]
    ))
}

/// The response with a freshly minted `<RequestId>` in its `<ErrorResponse>` envelope,
/// and the same id in the header AWS reports it in.
///
/// Status, headers and every other byte of the body are left exactly as the renderer
/// wrote them; a response that is not an awsQuery error envelope (or already carries a
/// request id) is returned untouched, so the success path cannot be affected.
pub fn with_request_id(mut resp: RtResponse) -> RtResponse {
    let request_id = new_request_id();
    let rewritten = {
        // An error envelope is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error envelope and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return resp;
        };
        match add_request_id(text, &request_id) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        let _ = resp.headers_mut().try_insert("content-length", length);
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    // Dropped rather than fatal if the HTTP layer cannot carry it: the document already
    // states the id, and a header is not worth failing a modelled error for.
    let _ = resp.headers_mut().try_insert(REQUEST_ID_HEADER, request_id);
    resp
}

/// The modelled operation the service's routing table names for this request, or `None`
/// when the request names none.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then adds the envelope's `<RequestId>` to the response and publishes the notifications
/// the call's own cluster events owe.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework still owns the whole
/// deserialize → validate → handle → serialize sequence (and every status, code and
/// message it answers with), and this wrapper owns one element of the envelope — plus the
/// one piece of work that belongs to NO single operation: a cache cluster's event is
/// published to the Amazon SNS topic its notification configuration names, and an event is
/// recorded by the lifecycle calls AND by every call that materialises a change the clock
/// made due, so the delivery is made here rather than repeated in each of them. It runs
/// after the response has been rendered, so it cannot change what this call answers.
pub struct RequestIdEnvelopeOperation<Eng: Send + Sync + 'static> {
    /// The shared engine this service's store lives in — the one the notifications are
    /// taken from. Held rather than reached through `svc`, which is engine-erased.
    engine: std::sync::Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> RequestIdEnvelopeOperation<Eng> {
    /// Build the wrapper over the shared engine, as `EmulatedService::new` does.
    pub fn new(engine: std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for RequestIdEnvelopeOperation<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let answered = match modeled_operation(svc, req) {
                Some(op) => with_request_id(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at. The
                // envelope element is NOT added here: that answer comes from
                // [`ElastiCacheDefinition::routing_error_response`], which already adds
                // it, so there is exactly one writer on this path.
                None => svc.definition().routing_error_response(req),
            };
            crate::state::deliver_cluster_notifications(&self.engine, ctx).await;
            answered
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is
        // the operation this wrapper delegates to, so its model-declared schemes decide
        // and this wrapper cannot weaken them.
        None
    }
}

/// This service's model-derived half, with the framework-level error renderers wrapped so
/// a routing failure, a transport failure and a pre-dispatch refusal answer in the same
/// envelope every other error does.
///
/// Identity, the routing table and the protocol come from the generated
/// [`crate::ServiceDefinition`] untouched — nothing model-derived is restated here.
pub struct ElastiCacheDefinition<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> ElastiCacheDefinition<Eng> {
    /// Build the definition from the shared engine, which the generated entry threads
    /// into every operation.
    pub fn new(engine: std::sync::Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }

    /// Whether the generated routing table can serve this request — the condition under
    /// which this service claims it for [`RequestIdEnvelopeOperation`].
    pub fn routes(&self, req: &RtRequest) -> bool {
        self.entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.entry.routing_table().get(key))
            .is_some()
    }
}

impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for ElastiCacheDefinition<Eng> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The element lands as the last child of the envelope — after `</Error>`, inside
    /// `</ErrorResponse>` — whether or not the root carries the service namespace.
    #[test]
    fn request_id_is_the_last_child_of_the_envelope() {
        for case in [
            "<?xml version=\"1.0\"?><ErrorResponse><Error><Type>Sender</Type>\
             <Code>CacheParameterGroupNotFound</Code><Message>no</Message></Error>\
             </ErrorResponse>",
            "<?xml version=\"1.0\"?><ErrorResponse xmlns=\"ns\"><Error><Type>Sender</Type>\
             <Code>InvalidParameterValue</Code><Message>no</Message></Error></ErrorResponse>",
        ] {
            let rewritten =
                add_request_id(case, "11112222-3333-4444-5555-666677778888").expect("rewritten");
            assert!(
                rewritten.ends_with(
                    "</Error><RequestId>11112222-3333-4444-5555-666677778888</RequestId>\
                     </ErrorResponse>"
                ),
                "case {case:?}: expected <RequestId> as the envelope's last child, got \
                 {rewritten:?}"
            );
        }
    }

    /// Nothing to add: a document that already carries a request id in the sibling
    /// position, and one that is not an error envelope at all, are both left alone.
    #[test]
    fn nothing_is_added_twice_and_nothing_else_is_touched() {
        for case in [
            "<ErrorResponse><Error/><RequestId>already</RequestId></ErrorResponse>",
            "<DescribeCacheParameterGroupsResponse><DescribeCacheParameterGroupsResult/>\
             <ResponseMetadata><RequestId>a</RequestId></ResponseMetadata>\
             </DescribeCacheParameterGroupsResponse>",
            "<DeleteCacheParameterGroupResponse><ResponseMetadata/>\
             </DeleteCacheParameterGroupResponse>",
        ] {
            assert!(
                add_request_id(case, "fresh").is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }

    /// A `<RequestId>` nested inside `<Error>` is a member of an error shape, not the
    /// envelope's element, so the envelope's own one is still added.
    #[test]
    fn a_request_id_inside_the_error_element_is_not_the_envelopes() {
        let case = "<ErrorResponse><Error><Code>X</Code><RequestId>inner</RequestId></Error>\
                    </ErrorResponse>";
        let rewritten = add_request_id(case, "outer").expect("rewritten");
        assert!(
            rewritten.ends_with("</Error><RequestId>outer</RequestId></ErrorResponse>"),
            "expected the envelope's own <RequestId> to be added, got {rewritten:?}"
        );
    }

    /// The escaping invariant this splice rests on, checked rather than only stated: a
    /// message that QUOTES the closing tags (as the renderer escapes any value carrying
    /// them) does not move where the element lands.
    #[test]
    fn an_escaped_closing_tag_in_the_message_does_not_move_the_element() {
        let case = "<ErrorResponse><Error><Type>Sender</Type><Code>InvalidParameterValue</Code>\
                    <Message>The marker &lt;/Error&gt;&lt;/ErrorResponse&gt; was invalid\
                    </Message></Error></ErrorResponse>";
        let rewritten = add_request_id(case, "fresh").expect("rewritten");
        assert!(
            rewritten.ends_with("</Error><RequestId>fresh</RequestId></ErrorResponse>"),
            "expected the element as the envelope's last child, got {rewritten:?}"
        );
        assert_eq!(
            rewritten.matches("<RequestId>").count(),
            1,
            "expected exactly one element added, got {rewritten:?}"
        );
    }

    /// Over a whole response: an error envelope grows the element AND the header AWS
    /// reports the same id in, while a response this writer leaves alone is returned
    /// byte for byte.
    #[test]
    fn a_response_reports_one_id_in_the_document_and_the_header() {
        // (case, body, whether the writer must rewrite it)
        let rows: [(&str, &str, bool); 3] = [
            (
                "an error envelope",
                "<?xml version=\"1.0\"?><ErrorResponse xmlns=\"ns\"><Error><Type>Sender</Type>\
                 <Code>CacheClusterNotFound</Code><Message>no</Message></Error></ErrorResponse>",
                true,
            ),
            (
                "an envelope that already carries the element",
                "<ErrorResponse><Error/><RequestId>already</RequestId></ErrorResponse>",
                false,
            ),
            (
                "a successful response",
                "<DescribeSnapshotsResponse><DescribeSnapshotsResult/>\
                 <ResponseMetadata><RequestId>a</RequestId></ResponseMetadata>\
                 </DescribeSnapshotsResponse>",
                false,
            ),
        ];

        for (case, body, rewritten) in rows {
            let response = RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(400).expect("400"),
                SdkBody::from(body),
            );
            let answered = with_request_id(response);
            let answered_body =
                std::str::from_utf8(answered.body().bytes().expect("an in-memory body"))
                    .expect("utf-8");
            let header = answered.headers().get(REQUEST_ID_HEADER);
            if rewritten {
                let ids: Vec<&str> = answered_body
                    .split("<RequestId>")
                    .skip(1)
                    .filter_map(|rest| rest.split("</RequestId>").next())
                    .collect();
                assert_eq!(
                    ids.len(),
                    1,
                    "case {case:?}: expected exactly one id in the document, got \
                     {answered_body:?}"
                );
                assert_eq!(
                    header,
                    Some(ids[0]),
                    "case {case:?}: expected the header to carry the document's id, got \
                     {header:?}"
                );
            } else {
                assert_eq!(
                    answered_body, body,
                    "case {case:?}: expected the response returned untouched"
                );
                assert_eq!(
                    header, None,
                    "case {case:?}: expected no header on a response this writer leaves \
                     alone, got {header:?}"
                );
            }
        }
    }

    /// The id this service mints names ONE request: two responses never report the same
    /// one, and it has the 8-4-4-4-12 form AWS answers with.
    #[test]
    fn each_request_id_is_fresh_and_well_formed() {
        let first = new_request_id();
        let second = new_request_id();
        assert_ne!(
            first, second,
            "expected two distinct request ids, got {first:?} twice"
        );
        for id in [&first, &second] {
            let groups: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(
                groups,
                vec![8, 4, 4, 4, 12],
                "expected the 8-4-4-4-12 form, got {id:?}"
            );
            assert!(
                id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
                "expected a hexadecimal id, got {id:?}"
            );
        }
    }
}
