// SPDX-License-Identifier: CC0-1.0

//! Which ENVELOPE this service answers a request its front door refused in.
//!
//! # The rule
//!
//! Two stacks front this service's operations, and a request refused before any handler ran
//! is answered by the one that fronts the operation it addressed:
//!
//! * a STREAM-ADDRESSED operation — one whose input names the stream by the
//!   `StreamName`/`StreamARN` pair — is fronted by the stream stack, which answers the
//!   GENERIC validation refusal: the code a constraint refusal carries on this protocol
//!   (`ValidationException`), the shape named bare in `x-amzn-errortype`, and the text in that
//!   envelope's own `message` member;
//! * every other operation answers this service's MODELLED `InvalidArgumentException` — "The
//!   value for this input parameter is invalid.",
//!   `smithy_query(query="service_info")`'s common errors — with its text in the `Message`
//!   member every error shape of this model declares ([`super::error_message_member`]).
//!
//! The WORDS are the same either way, and they are not chosen here: the refusal seam states
//! them for both stacks ([`crate::service::invalid_argument_refusal`]) because it is handed
//! the violations alone and cannot see which operation they belong to. That is exactly why
//! the ENVELOPE — the code, the shape the header names and the member — is stated here, at
//! the one seam that sees both the operation and the whole rendered response.
//!
//! WHICH side an operation falls on is recorded for the stream-addressed operations that have
//! a recording of a refused request (`DescribeStream`, `GetDataEndpoint`, `TagStream`,
//! `ListTagsForStream`) and for three on the other side (`TagResource`, `UntagResource`,
//! `ListStreams`); every other operation follows the MODEL, by the one property those two sets
//! differ in — whether the input names a stream by the selector pair — rather than by a row of
//! its own. The signaling-channel readers are therefore on the modelled side because their
//! input names no stream, which is an inference from that rule and not an observation of them.
//!
//! Only a refusal the framework's own renderer wrote is touched. An error a HANDLER returned
//! is serialized from the model's own shape, so it already names that shape and carries its
//! `Message` member, whichever operation answered; a success, and a failure whose class this
//! model does not declare (the scaffold's 501), keep every byte their renderer wrote.
//!
//! The structural home for a per-stack envelope is the framework's refusal renderer, which
//! writes one envelope for every service; that renderer is in the agent-immutable `vendor/`
//! tree, so this service states it at the operation wrapper in
//! [`super::error_message_member`], which is where this layer is applied.

use aws_smithy_runtime_api::http::Response as RtResponse;

/// The operations the stream stack fronts: every one whose input names the stream it
/// addresses by the `StreamName`/`StreamARN` pair, which is what makes a call one that stack
/// routes.
///
/// From the model's own input shapes — the operations carrying BOTH of those optional members
/// (`smithy_query(query="digest")` for the bound operations, then each one's input) — so an
/// operation the model gains with that pair joins this set for the same reason the ones below
/// are in it. The operations addressing a stream some other way (`CreateStream` and
/// `DeleteStream`, by one identifier alone), a signaling channel, a resource ARN of any class
/// (`TagResource`, `UntagResource`, `ListTagsForResource`) or no resource at all
/// (`ListStreams`) are NOT in it.
const STREAM_SELECTOR_OPERATIONS: [&str; 17] = [
    "DeleteEdgeConfiguration",
    "DescribeEdgeConfiguration",
    "DescribeImageGenerationConfiguration",
    "DescribeMappedResourceConfiguration",
    "DescribeNotificationConfiguration",
    "DescribeStream",
    "DescribeStreamStorageConfiguration",
    "GetDataEndpoint",
    "ListTagsForStream",
    "StartEdgeConfigurationUpdate",
    "TagStream",
    "UntagStream",
    "UpdateDataRetention",
    "UpdateImageGenerationConfiguration",
    "UpdateNotificationConfiguration",
    "UpdateStream",
    "UpdateStreamStorageConfiguration",
];

/// The code a constraint refusal carries on this protocol, which the framework's own renderer
/// writes and which the stream stack's refusal keeps.
pub const GENERIC_VALIDATION_EXCEPTION: &str = "ValidationException";

/// The MODELLED error this service refuses an input value with, which the other stack's
/// refusal carries instead ([`crate::service::INVALID_ARGUMENT_EXCEPTION`]).
use crate::service::INVALID_ARGUMENT_EXCEPTION;

/// Whether `operation` is fronted by the stream stack, and so whether a refusal raised before
/// its handler ran is answered in the generic validation envelope.
#[must_use]
pub fn stream_stack_fronts(operation: &str) -> bool {
    STREAM_SELECTOR_OPERATIONS.contains(&operation)
}

/// `resp` as this service answers it, for a response the operation `operation` produced.
///
/// The stream stack's own envelope is the one the framework rendered, so a response of an
/// operation that stack fronts is returned untouched; every other operation's refusal is
/// restated as the modelled exception — the shape its body and its header name
/// (`service_common::error_type_locator`), and then the member its text travels in
/// ([`super::error_message_member`]).
///
/// A response that is not a refusal the framework's own renderer wrote is returned untouched
/// either way: an error a HANDLER returned is serialized from the model's own shape, so it
/// names that shape and carries its `Message` member already, and a success keeps every byte.
#[must_use]
pub fn with_refusal_envelope(resp: RtResponse, operation: &str) -> RtResponse {
    if stream_stack_fronts(operation) {
        return resp;
    }
    let restated = service_common::error_type_locator::with_restated_error_shape(
        resp,
        GENERIC_VALIDATION_EXCEPTION,
        INVALID_ARGUMENT_EXCEPTION,
    );
    super::error_message_member::with_modelled_message_member(restated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;
    use aws_smithy_types::body::SdkBody;
    use serde_json::Value;
    // The two places one restatement writes the error shape.
    use service_common::error_type_locator::{ERROR_TYPE_HEADER, SHAPE_DISCRIMINATOR};

    /// The generic envelope's own member, which the stream stack's refusal keeps.
    const RENDERED_MEMBER: &str = "message";

    /// The member every error shape of this model declares.
    const MODELLED_MEMBER: &str = "Message";

    /// One response at `status`, carrying `body` and naming `error_type`.
    fn response(status: u16, body: &str, error_type: &str) -> RtResponse {
        let mut resp = RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        );
        resp.headers_mut().insert(ERROR_TYPE_HEADER, error_type.to_string());
        resp
    }

    /// The body of `resp` as JSON.
    fn decoded(resp: &RtResponse) -> Value {
        serde_json::from_slice(resp.body().bytes().expect("an in-memory body"))
            .expect("the body is JSON")
    }

    /// The generic refusal the framework's renderer writes, with AWS's own words in it.
    fn generic_refusal() -> String {
        format!(
            r#"{{"{SHAPE_DISCRIMINATOR}":"{GENERIC_VALIDATION_EXCEPTION}","{RENDERED_MEMBER}":"1 validation error detected: Value at 'streamARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:"}}"#
        )
    }

    /// RULE: one refusal, two envelopes — an operation the stream stack fronts keeps the
    /// generic one (code, header and `message` member), and every other operation answers the
    /// modelled `InvalidArgumentException` with its text in `Message`.
    #[test]
    fn each_stack_answers_a_refusal_in_its_own_envelope() {
        // (case, the operation, whether the stream stack fronts it)
        let rows: &[(&str, &str, bool)] = &[
            ("a stream read", "DescribeStream", true),
            ("the data endpoint", "GetDataEndpoint", true),
            ("a stream's tag write", "TagStream", true),
            ("a stream's tag listing", "ListTagsForStream", true),
            ("a stream's retention update", "UpdateDataRetention", true),
            ("the generic tagging surface", "TagResource", false),
            ("its untagging half", "UntagResource", false),
            ("a listing that names no stream", "ListStreams", false),
            ("a channel read", "DescribeSignalingChannel", false),
            ("a create", "CreateStream", false),
        ];

        for (case, operation, fronted) in rows {
            let answered = with_refusal_envelope(
                response(400, &generic_refusal(), GENERIC_VALIDATION_EXCEPTION),
                operation,
            );
            let named = answered
                .headers()
                .get(ERROR_TYPE_HEADER)
                .unwrap_or_default()
                .to_string();
            let document = decoded(&answered);
            let (expected_shape, expected_member, absent_member) = if *fronted {
                (
                    GENERIC_VALIDATION_EXCEPTION,
                    RENDERED_MEMBER,
                    MODELLED_MEMBER,
                )
            } else {
                (
                    INVALID_ARGUMENT_EXCEPTION,
                    MODELLED_MEMBER,
                    RENDERED_MEMBER,
                )
            };
            assert_eq!(
                named, expected_shape,
                "case {case:?} on {operation}: expected x-amzn-errortype {expected_shape:?}, got \
                 {named:?}"
            );
            assert_eq!(
                document.get(SHAPE_DISCRIMINATOR).and_then(Value::as_str),
                Some(expected_shape),
                "case {case:?} on {operation}: expected the body to name {expected_shape:?}, got \
                 {document}"
            );
            assert!(
                document
                    .get(expected_member)
                    .and_then(Value::as_str)
                    .is_some_and(|text| text.starts_with("1 validation error detected: ")),
                "case {case:?} on {operation}: expected the text under {expected_member:?}, got \
                 {document}"
            );
            assert!(
                document.get(absent_member).is_none(),
                "case {case:?} on {operation}: the text must be carried under ONE member, got \
                 {document}"
            );
        }
    }

    /// RULE: a response no refusal renderer wrote is answered as its renderer wrote it,
    /// whichever stack fronts the operation — a success, an error a handler serialized from
    /// the model's own shape, and a failure whose class this model does not declare.
    #[test]
    fn a_response_this_rule_does_not_own_is_answered_unchanged() {
        // (case, status, the body, the shape its renderer named)
        let rows: &[(&str, u16, &str, &str)] = &[
            (
                "a success",
                200,
                r#"{"StreamInfo":{"Status":"ACTIVE","Version":"aBcD"}}"#,
                "",
            ),
            (
                "a modelled error a handler serialized",
                400,
                r#"{"__type":"InvalidArgumentException","Message":"One of StreamName or StreamARN must be provided, but not both."}"#,
                "InvalidArgumentException",
            ),
            (
                "another modelled error a handler serialized",
                404,
                r#"{"__type":"ResourceNotFoundException","Message":"The requested stream is not found or not active."}"#,
                "ResourceNotFoundException",
            ),
            (
                "the scaffold's 501, a class this model does not declare",
                501,
                r#"{"__type":"NotImplemented","message":"not implemented in this emulator"}"#,
                "NotImplemented",
            ),
        ];

        for (case, status, body, shape) in rows {
            for operation in ["DescribeStream", "TagResource"] {
                let answered =
                    with_refusal_envelope(response(*status, body, shape), operation);
                assert_eq!(
                    answered.body().bytes().expect("an in-memory body"),
                    body.as_bytes(),
                    "case {case:?} on {operation}: expected the body unchanged"
                );
                assert_eq!(
                    answered.headers().get(ERROR_TYPE_HEADER),
                    Some(*shape),
                    "case {case:?} on {operation}: expected the header unchanged, got {:?}",
                    answered.headers().get(ERROR_TYPE_HEADER)
                );
            }
        }
    }
}
