// SPDX-License-Identifier: CC0-1.0

//! The media types this service's responses are announced under — there are two.
//!
//! # What the wire shows
//!
//! The model declares `aws.protocols#awsJson1_0`
//! (`smithy_query(query="service_info",model_path="services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json")`),
//! which implies `application/x-amz-json-1.0`, and the framework's renderers stamp exactly
//! that. The recorded wire shows that value on some of this service's responses and
//! `application/x-amz-json-1.1` on others, and the split is visible INSIDE one recorded
//! session rather than across sessions — one fixture's refusal of an out-of-range page size
//! is recorded under `1.0` while the very next call of that same operation, a success, is
//! recorded under `1.1`. So this service answers through two announcers:
//!
//! * the DASHBOARD and SCHEDULED-REPORT operations are fronted by a dispatcher announcing
//!   `application/x-amz-json-1.1` on what it returns — their successes, and the errors
//!   their handlers raise about stored state (a resource that does not exist, say);
//! * a refusal that reports the REQUEST ITSELF as invalid — the modelled
//!   `ValidationException` — is answered by the request validator in front of that
//!   dispatcher, and announces the protocol-implied `application/x-amz-json-1.0`. So do the
//!   other responses rendered in front of it: an unroutable request, a transport-level
//!   failure, the authentication refusal;
//! * the RESOURCE-TAGGING operations announce the protocol-implied media type throughout,
//!   on every status.
//!
//! Which layer renders a refusal about the request is the EMULATOR's business and does not
//! change what it announces: the boundary validator refuses what the model's constraints
//! express, and a handler re-applies the ones it cannot (a `@pattern` the boundary's regular
//! expression engine will not compile — see `crate::state::text_pattern_refusal`), so the
//! question below is asked of the refusal the client sees rather than of the seam that
//! produced it. It is
//! [`service_common::response_media_type::not_an_input_validation_refusal`],
//! because it is not this service's own question.
//!
//! The two lists are therefore read on different things, and each on what the wire record
//! shows: the tagging family is a fact of the OPERATION, so it is read off the routing key
//! before the walk runs, while the validator's refusal is a fact of the RESPONSE. The
//! responses rendered outside the walk — an unroutable request, a transport-level failure,
//! the authentication refusal — reach neither list: no operation of this service ran, so
//! they keep what their own renderer stamped, which is the protocol-implied value. A
//! failure the framework renders INSIDE the walk of an operation the dispatcher fronts and
//! that is not a modelled constraint refusal (a body the codec cannot read) is announced as
//! the dispatcher's, because the recorded wire shows no such response under the other value.
//!
//! The REQUEST side is untouched by all of it: the media type a caller sends is never
//! re-read, requests still route by `X-Amz-Target: AWSBCMDashboardsService.<Operation>`, and
//! the awsJson `1.0` and `1.1` codecs are the same `JsonCodec` over the same body, so a call
//! sent under either one reads byte for byte.
//!
//! # Where the rule lives
//!
//! Not here. The mechanism — the one function that re-announces a response
//! ([`ResponseMediaType::announce`]), the predicate type a service states its announcers
//! with and the operation-seam wrapper that carries both around the centralized walk
//! ([`ResponseMediaTypeOperation`](service_common::response_media_type::ResponseMediaTypeOperation))
//! — is [`service_common::response_media_type`], the tree's one statement of it. This
//! module holds only what is this SERVICE's: the pair its dispatcher announces across,
//! which operations that dispatcher fronts, and which responses it produced — named once so
//! every seam in [`crate::service`] reads the same values.
//! `tests/all/response_media_type.rs` pins both announcers over the wire.

use aws_smithy_runtime_api::http::Response as RtResponse;
use service_common::response_media_type::{ResponseMediaType, AWS_JSON_1_0, AWS_JSON_1_1};

/// The pair a response of the dashboard and scheduled-report dispatcher is re-announced
/// across: the `application/x-amz-json-1.0` the model's protocol trait implies — stamped by
/// the framework's own renderers, and the ONLY value replaced — and the
/// `application/x-amz-json-1.1` that dispatcher announces.
pub const DISPATCHER_RESPONSE_MEDIA_TYPE: ResponseMediaType =
    ResponseMediaType::new(AWS_JSON_1_0, AWS_JSON_1_1);

/// The resource-tagging operations this service models — the whole family, not a chosen few
/// (source: `smithy_query(query="list_operations")`, which binds these three and no other
/// tagging operation). Their responses are not the dispatcher's.
const TAGGING_OPERATIONS: [&str; 3] = ["ListTagsForResource", "TagResource", "UntagResource"];

/// Whether `routing_key` — the operation name the protocol reads out of `X-Amz-Target` —
/// names an operation the dashboard and scheduled-report dispatcher fronts, as opposed to a
/// resource-tagging one.
pub fn fronted_by_the_dispatcher(routing_key: &str) -> bool {
    !TAGGING_OPERATIONS.contains(&routing_key)
}

/// Whether `resp` is a response the dispatcher produced, rather than one the request
/// validator in front of it answered.
///
/// The validator's refusal is the one reporting the REQUEST ITSELF as invalid, recognised
/// from the body the client receives wherever the emulator rendered it; the question is
/// `service_common`'s, so this service and every other one on that wire ask it the same way.
pub fn produced_by_the_dispatcher(resp: &RtResponse) -> bool {
    service_common::response_media_type::not_an_input_validation_refusal(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

    /// The response header carrying the media type.
    const CONTENT_TYPE: &str = "content-type";

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

    /// A response carrying `body` under `status`, as a layer of the walk renders one.
    fn answered(status: u16, body: &str) -> RtResponse {
        RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status code"),
            SdkBody::from(body.as_bytes().to_vec()),
        )
    }

    /// The protocol-implied media type becomes the one the dispatcher announces; every
    /// other value is left alone.
    ///
    /// The cases span what a producer can put in the header: the protocol-implied media
    /// type (what the framework's renderers stamp), the value the dispatcher announces
    /// (already right, so unchanged), a deliberately different media type (an event-stream
    /// frame), and no header at all.
    #[test]
    fn only_the_protocol_implied_media_type_is_reannounced() {
        let cases: [(Option<&str>, Option<&str>); 4] = [
            (Some(AWS_JSON_1_0), Some(AWS_JSON_1_1)),
            (Some(AWS_JSON_1_1), Some(AWS_JSON_1_1)),
            (
                Some("application/vnd.amazon.eventstream"),
                Some("application/vnd.amazon.eventstream"),
            ),
            (None, None),
        ];

        for (given, expected) in cases {
            let actual =
                media_type(&DISPATCHER_RESPONSE_MEDIA_TYPE.announce(response(given)));
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {given:?}: expected content-type {expected:?}, got {actual:?}"
            );
        }
    }

    /// The dispatcher fronts every operation this service models except the resource-tagging
    /// family.
    #[test]
    fn the_dispatcher_fronts_everything_but_the_tagging_operations() {
        // Every operation the service binds, read from
        // `smithy_query(query="list_operations")`, split into the two families.
        let tagging = ["ListTagsForResource", "TagResource", "UntagResource"];
        let dispatched = [
            "CreateDashboard",
            "DeleteDashboard",
            "GetDashboard",
            "ListDashboards",
            "UpdateDashboard",
            "CreateScheduledReport",
            "DeleteScheduledReport",
            "ExecuteScheduledReport",
            "GetScheduledReport",
            "ListScheduledReports",
            "UpdateScheduledReport",
            "GetResourcePolicy",
        ];

        for operation in tagging {
            assert!(
                !fronted_by_the_dispatcher(operation),
                "case {operation:?}: a resource-tagging operation is not fronted by the \
                 dispatcher, got true"
            );
        }
        for operation in dispatched {
            assert!(
                fronted_by_the_dispatcher(operation),
                "case {operation:?}: a dashboard or scheduled-report operation is fronted \
                 by the dispatcher, got false"
            );
        }
    }

    /// A refusal naming the modelled `ValidationException` is the request validator's;
    /// every other response of the walk is the dispatcher's.
    ///
    /// The rows span the envelopes a client can receive: the validation refusal qualified
    /// and unqualified, another client error of the same status, a success, a server error,
    /// an empty body and a body that is not JSON at all.
    #[test]
    fn only_the_validation_refusal_is_the_validators() {
        let cases: [(&str, RtResponse, bool); 8] = [
            (
                "the validation refusal",
                answered(400, r#"{"__type":"ValidationException","message":"1 validation error detected"}"#),
                false,
            ),
            (
                "the validation refusal, namespace-qualified",
                answered(
                    400,
                    r#"{"__type":"com.amazonaws.bcmdashboards#ValidationException","message":"x"}"#,
                ),
                false,
            ),
            (
                "a refusal about stored state, same status",
                answered(400, r#"{"__type":"ResourceNotFoundException","message":"x"}"#),
                true,
            ),
            (
                "a client error the framework renders before the operation",
                answered(400, r#"{"__type":"SerializationException","message":"x"}"#),
                true,
            ),
            (
                "another client error, a different status",
                answered(402, r#"{"__type":"ServiceQuotaExceededException","message":"x"}"#),
                true,
            ),
            ("a success", answered(200, r#"{"dashboards":[]}"#), true),
            (
                "a server error",
                answered(500, r#"{"__type":"InternalServerException","message":"x"}"#),
                true,
            ),
            ("a body that is not the JSON envelope", answered(400, "not json"), true),
        ];

        for (case, resp, expected) in cases {
            let actual = produced_by_the_dispatcher(&resp);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected produced_by_the_dispatcher {expected}, got {actual}"
            );
        }
    }
}
