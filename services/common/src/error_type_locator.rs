// SPDX-License-Identifier: CC0-1.0

//! The TWO places a rendered JSON failure names its error SHAPE — the body's `__type`
//! discriminator and the `x-amzn-errortype` header a REST client resolves the shape from —
//! and the ONE restatement of that shape, for a service whose wire names a different shape
//! there from the one the layer that rendered the failure wrote.
//!
//! # Why a service needs this
//!
//! The framework's pre-handler renderers pick the error shape themselves: a constraint
//! refusal is rendered under the generic validation shape for every service, and
//! [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal) is where a
//! service states its own instead. That seam is handed the violations ALONE, so a service
//! whose wire answers one shape for some operations and another for the rest — because two
//! stacks front its operations, each with its own refusal envelope — cannot decide there: it
//! has to restate the shape later, where the operation that answered is known.
//!
//! Restating it means writing it in BOTH places, or a client that reads the header and one
//! that reads the body disagree about what was refused. That is the whole of this module;
//! WHICH shape replaces which is the service's fact, read off its recordings, so the caller
//! states the pair.
//!
//! # What it does not touch
//!
//! The status, the detail text and every other member stay with the renderer that wrote them
//! (the member that text travels in is [`crate::error_detail_member`]'s), and a success is
//! returned byte for byte. Only a failure whose body names the `from` shape is rewritten, so
//! an error a handler serialized from its model's own shape, and a failure of any other
//! class, keep the shape they named.

use aws_smithy_runtime_api::http::Response as RtResponse;
use aws_smithy_types::body::SdkBody;
use serde_json::Value;

/// The header a REST-JSON client resolves a failure's error shape from.
pub const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The member a JSON error envelope names that same shape in.
pub const SHAPE_DISCRIMINATOR: &str = "__type";

/// The lowest status a JSON error envelope is rendered under: below it a response is a
/// success and names no error shape at all.
const FIRST_ERROR_STATUS: u16 = 400;

/// The bare error SHAPE a rendered value names, which is what both places are matched on.
///
/// A value may arrive namespaced (`com.amazonaws.example#ValidationException`); the namespace
/// is not part of the shape's name, and no SDK reads it as one.
#[must_use]
pub fn shape_named(raw: &str) -> &str {
    raw.trim().rsplit('#').next().unwrap_or(raw).trim()
}

/// `body` with the error shape it names replaced by `to`, or `None` when there is nothing to
/// restate: the body is not a JSON object, or the shape it names is not `from`.
///
/// Every other member is written back as it was, so only the discriminator moves.
#[must_use]
pub fn restated_error_shape(body: &[u8], from: &str, to: &str) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    let named = object.get(SHAPE_DISCRIMINATOR).and_then(Value::as_str)?;
    if shape_named(named) != from {
        return None;
    }
    object.insert(SHAPE_DISCRIMINATOR.to_string(), Value::String(to.to_string()));
    serde_json::to_vec(&document).ok()
}

/// `resp` as the serving service answers it: a failure that named `from` names `to` instead,
/// in its body and in its [`ERROR_TYPE_HEADER`] alike ([`restated_error_shape`]).
///
/// A response that is not a failure, or whose body does not name `from`, is returned
/// untouched — status, headers and every byte of the body exactly as the renderer wrote them.
/// A header value the HTTP layer will not carry is left as it was rather than turned into a
/// transport failure, which on an error path would replace a modelled 4xx with a 500 an SDK
/// retries.
#[must_use]
pub fn with_restated_error_shape(mut resp: RtResponse, from: &str, to: &str) -> RtResponse {
    if resp.status().as_u16() < FIRST_ERROR_STATUS {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable here; a
        // streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match restated_error_shape(bytes, from, to) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        // A decimal length is always a valid header value, so this cannot fail today; if it
        // ever did, the header is REMOVED rather than left describing the body before the
        // rewrite, so the two can never disagree.
        if resp
            .headers_mut()
            .try_insert("content-length", length)
            .is_err()
        {
            resp.headers_mut().remove("content-length");
        }
    }
    let _ = resp
        .headers_mut()
        .try_insert(ERROR_TYPE_HEADER, to.to_string());
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;

    /// The shape a framework refusal renderer names, and the one a service restates it as.
    const RENDERED: &str = "ValidationException";
    const SERVICE_OWN: &str = "BadRequestException";

    /// One response at `status`, carrying `body` and naming `error_type` in the header.
    fn response(status: u16, body: &str, error_type: &str) -> RtResponse {
        let mut resp = RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        );
        resp.headers_mut()
            .insert(ERROR_TYPE_HEADER, error_type.to_string());
        resp
    }

    /// The body of `resp` as JSON.
    fn decoded(resp: &RtResponse) -> Value {
        serde_json::from_slice(resp.body().bytes().expect("an in-memory body"))
            .expect("the body is JSON")
    }

    /// The shape moves in BOTH places, whether the body named it bare or namespaced, and
    /// every other member is written back as it was.
    #[test]
    fn the_shape_is_restated_in_the_body_and_in_the_header() {
        for (case, body) in [
            (
                "a bare shape",
                format!(r#"{{"__type":"{RENDERED}","message":"no","retryable":false}}"#),
            ),
            (
                "a namespaced shape",
                format!(
                    r#"{{"__type":"com.amazonaws.example#{RENDERED}","message":"no","retryable":false}}"#
                ),
            ),
        ] {
            let answered =
                with_restated_error_shape(response(400, &body, RENDERED), RENDERED, SERVICE_OWN);
            let document = decoded(&answered);
            assert_eq!(
                document.get(SHAPE_DISCRIMINATOR).and_then(Value::as_str),
                Some(SERVICE_OWN),
                "case {case:?}: expected the body to name {SERVICE_OWN:?}, got {document}"
            );
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(SERVICE_OWN),
                "case {case:?}: expected the header to name {SERVICE_OWN:?}, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
            assert_eq!(
                document.get("message").and_then(Value::as_str),
                Some("no"),
                "case {case:?}: the detail text must survive, got {document}"
            );
            assert_eq!(
                document.get("retryable"),
                Some(&Value::Bool(false)),
                "case {case:?}: every other member must survive, got {document}"
            );
        }
    }

    /// Nothing to restate: a success, a failure naming another shape, a body that is not a
    /// JSON object, one that is not JSON at all, and one naming no shape — each is answered
    /// byte for byte, header included.
    #[test]
    fn a_response_naming_another_shape_is_answered_unchanged() {
        for (case, status, body, named) in [
            (
                "a success",
                200,
                format!(r#"{{"__type":"{RENDERED}"}}"#),
                RENDERED,
            ),
            (
                "another shape",
                404,
                r#"{"__type":"NotFoundException","Message":"gone"}"#.to_string(),
                "NotFoundException",
            ),
            (
                "not an object",
                400,
                r#"["ValidationException"]"#.to_string(),
                RENDERED,
            ),
            ("not JSON", 400, "<Error/>".to_string(), RENDERED),
            (
                "no shape at all",
                400,
                r#"{"message":"no"}"#.to_string(),
                RENDERED,
            ),
        ] {
            let answered =
                with_restated_error_shape(response(status, &body, named), RENDERED, SERVICE_OWN);
            assert_eq!(
                answered.body().bytes().expect("an in-memory body"),
                body.as_bytes(),
                "case {case:?}: expected the body unchanged"
            );
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(named),
                "case {case:?}: expected the header unchanged, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
        }
    }
}
