// SPDX-License-Identifier: CC0-1.0

//! The document this service answers a FAILURE with, and the id every response names the
//! request it answered by.
//!
//! A client of this service reads a failure out of four members of the response document:
//! the error's `Code`, its `Message`, the modelled exception's remaining members under
//! `Details`, and the `RequestID` of the call that failed — which is what the recorded
//! `ivschat` failures carry, and what the sibling Amazon IVS endpoints carry for the
//! members they have. The protocol's own renderer writes none of them: it writes the
//! `restJson1` discriminator (`__type` in the body and the `x-amzn-errortype` header) and
//! the modelled members at the top level of the document, under the names the model
//! declares. So the document is COMPLETED here, never rewritten:
//!
//! * `Code` is the code the renderer already put in `x-amzn-errortype` — model-derived,
//!   from the modelled error shape the operation declares — falling back to the body's own
//!   `__type` when a renderer wrote only that;
//! * `Message` is the text the renderer wrote under the key an SDK error parser reads,
//!   built by `crate::state`'s message rules;
//! * `Details` holds the modelled exception's OTHER members exactly as the renderer wrote
//!   them (`resourceId`/`resourceType` for a not-found, `reason`/`fieldList` for a
//!   validation refusal), so nothing in it is a second rendering of a value;
//! * `RequestID` is the id this response names its request by, the same value the
//!   `x-amzn-requestid` header carries.
//!
//! The id is a property of the RESPONSE, not of any modelled shape: no output and no error
//! shape of this model declares it, so no handler can carry it, and a failure must carry
//! one exactly as a success does. The model says which header it goes in:
//! `cors.additionalExposedHeaders` (`smithy_query(query="service_info")`) publishes
//! `x-amzn-requestid`, `x-amzn-errortype` and `x-amzn-errormessage`, so those are the
//! headers this service sends.
//!
//! Everything else is left exactly as the renderer wrote it: the status, every member
//! already in the document, every other header, and the whole success path apart from the
//! one header. A member or header already present is never overwritten, so this is
//! idempotent and two layers of one service cannot state two codes, two texts or two ids
//! for one call.
//!
//! The two sibling IVS endpoints state the same family rule in their own
//! `src/service.rs` (for the three members their recordings carry); migrating all three
//! onto one home belongs to their own issues, so nothing here reaches into them.

use aws_smithy_runtime_api::http::Response as RtResponse;
use serde_json::{Map, Value};

/// The lowest status that makes a response a failure, and so the responses the error
/// document belongs on.
const FIRST_ERROR_STATUS: u16 = 400;

/// The response header AWS names the request it answers in, for this protocol.
pub const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The response header the protocol serializer writes an error's CODE in, and the one
/// `restJson1` resolves an error shape from. Read here (never written) so the code in the
/// document is the model-derived one and not a second derivation of it.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The response header AWS states an error's text in, beside the code.
const ERROR_MESSAGE_HEADER: &str = "x-amzn-errormessage";

/// The body key an SDK error parser reads an error's text under.
const SDK_MESSAGE_KEY: &str = "message";

/// The body key the protocol serializer writes the error shape name under.
const SDK_TYPE_KEY: &str = "__type";

/// The four members of the error document this service answers a failure with.
const DOCUMENT_CODE_KEY: &str = "Code";
const DOCUMENT_MESSAGE_KEY: &str = "Message";
const DOCUMENT_DETAILS_KEY: &str = "Details";
const DOCUMENT_REQUEST_ID_KEY: &str = "RequestID";

/// Mint the id ONE response names the request it answers with.
///
/// The canonical hyphenated form of a version-4 UUID, which is the form an AWS request id
/// takes. The VALUE is per-response and is not a requirement: nothing can predict it and no
/// caller may depend on a particular one. What it must do is name ONE response, which is
/// what the randomness gives it. It is an identifier for correlation, never a secret:
/// nothing authenticates with it, and nothing the REQUEST carried goes into it.
///
/// The randomness is the `uuid` crate's v4 generator, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is `uuid::Uuid::new_v4()` rendered
/// canonically) — a crate this service ALREADY depends on — rather than by declaring a
/// second path to the same crate or hand-rolling one.
fn new_request_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// One failure's completed document and the text it states, so the header and the body
/// carry the same text and not two renderings of it.
struct RestatedError {
    body: String,
    message: String,
}

/// The members of a rendered error document that are NOT part of the modelled exception's
/// detail: the protocol's own discriminator, the text, and the four members this module
/// completes the document with.
fn is_envelope_member(key: &str) -> bool {
    matches!(
        key,
        SDK_TYPE_KEY
            | SDK_MESSAGE_KEY
            | DOCUMENT_CODE_KEY
            | DOCUMENT_MESSAGE_KEY
            | DOCUMENT_DETAILS_KEY
            | DOCUMENT_REQUEST_ID_KEY
    )
}

/// The error document this service answers `resp` with, when `resp` is a failure carrying a
/// JSON body: the document the renderer already wrote, with the code, the text, the
/// modelled detail and the request id added under the names a client of this service reads
/// them by.
///
/// `None` when there is nothing to complete: a success, a response with no body (the
/// modelled 204 of `DeleteRoom`, say), a streamed body, a body that is not a JSON object,
/// or an error whose code cannot be read at all — in which case the response goes out
/// exactly as the renderer wrote it rather than with a document this service made up.
fn error_document(resp: &RtResponse, request_id: &str) -> Option<RestatedError> {
    if resp.status().as_u16() < FIRST_ERROR_STATUS {
        return None;
    }
    // An error body is built in memory by its renderer, so the bytes are readable here; a
    // streamed body is not an error document and is left alone.
    let bytes = resp.body().bytes()?;
    let mut document: Map<String, Value> = serde_json::from_slice(bytes).ok()?;
    let code = resp
        .headers()
        .get(ERROR_TYPE_HEADER)
        .map(str::to_string)
        .or_else(|| {
            document
                .get(SDK_TYPE_KEY)
                .and_then(Value::as_str)
                .map(str::to_string)
        })?;
    let message = document
        .get(SDK_MESSAGE_KEY)
        .or_else(|| document.get(DOCUMENT_MESSAGE_KEY))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // The modelled exception's own members, exactly as the renderer wrote them. An error
    // shape whose only member is its text — and a framework-rendered failure, which has no
    // modelled shape at all — carries no detail, so the member is absent rather than empty.
    let details: Map<String, Value> = document
        .iter()
        .filter(|(key, _)| !is_envelope_member(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    document
        .entry(DOCUMENT_CODE_KEY)
        .or_insert_with(|| Value::String(code));
    if !message.is_empty() {
        document
            .entry(DOCUMENT_MESSAGE_KEY)
            .or_insert_with(|| Value::String(message.clone()));
    }
    if !details.is_empty() {
        document
            .entry(DOCUMENT_DETAILS_KEY)
            .or_insert_with(|| Value::Object(details));
    }
    document
        .entry(DOCUMENT_REQUEST_ID_KEY)
        .or_insert_with(|| Value::String(request_id.to_string()));
    Some(RestatedError {
        body: Value::Object(document).to_string(),
        message,
    })
}

/// The response as this service sends it: naming the request it answers, and — on a failure
/// — stating the error's code, text, modelled detail and request id in the document a
/// client of this service reads a failure out of.
///
/// The status and every member the renderer wrote are left exactly as they were, and a
/// header already present is left alone, so the one writer of each is whoever wrote it
/// first — including the id itself, which is read off the response when a layer below
/// already named the request, so one call is never named by two ids. Every header insert is
/// FALLIBLE: a header the HTTP layer cannot represent is dropped rather than turned into a
/// transport failure, which on the error path would replace a modelled 4xx with a 500 an
/// SDK retries.
pub fn with_response_envelope(mut resp: RtResponse) -> RtResponse {
    let request_id = resp
        .headers()
        .get(REQUEST_ID_HEADER)
        .map(str::to_string)
        .unwrap_or_else(new_request_id);
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
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;
    use aws_smithy_types::body::SdkBody;

    /// A response with `status` and `body`, carrying the discriminator header the protocol
    /// renderer writes beside a modelled error.
    fn answered(status: u16, body: &str, error_type: Option<&str>) -> RtResponse {
        let mut resp = RtResponse::new(
            StatusCode::try_from(status).expect("a status"),
            SdkBody::from(body.to_owned()),
        );
        if let Some(error_type) = error_type {
            resp.headers_mut()
                .insert(ERROR_TYPE_HEADER, error_type.to_owned());
        }
        resp
    }

    /// The document a response carries.
    fn document(resp: &RtResponse) -> Map<String, Value> {
        serde_json::from_slice(resp.body().bytes().expect("in-memory bytes"))
            .expect("a JSON document")
    }

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
                groups.iter().all(|group| group
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
                "case {case}: expected lowercase hex, got {id:?}"
            );
            assert!(
                !seen.contains(&id),
                "case {case}: expected a fresh id, got the already-minted {id:?}"
            );
            seen.push(id);
        }
    }

    /// A modelled failure's document is completed with the code, the text, the modelled
    /// detail and the request id, and keeps every member the renderer wrote.
    #[test]
    fn a_modelled_failure_carries_the_whole_document() {
        let rendered = r#"{"__type":"ResourceNotFoundException","message":"Resource: arn not found","resourceId":"arn","resourceType":"ROOM"}"#;
        let answer = with_response_envelope(answered(
            404,
            rendered,
            Some("ResourceNotFoundException"),
        ));
        let document = document(&answer);
        assert_eq!(
            document.get(DOCUMENT_CODE_KEY).and_then(Value::as_str),
            Some("ResourceNotFoundException"),
            "got {document:?}"
        );
        assert_eq!(
            document.get(DOCUMENT_MESSAGE_KEY).and_then(Value::as_str),
            Some("Resource: arn not found"),
            "got {document:?}"
        );
        assert_eq!(
            document.get(DOCUMENT_DETAILS_KEY),
            Some(&serde_json::json!({"resourceId": "arn", "resourceType": "ROOM"})),
            "got {document:?}"
        );
        assert!(
            document
                .get(DOCUMENT_REQUEST_ID_KEY)
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "got {document:?}"
        );
        assert_eq!(
            answer.headers().get(REQUEST_ID_HEADER),
            document
                .get(DOCUMENT_REQUEST_ID_KEY)
                .and_then(Value::as_str),
            "the header and the document must name one id"
        );
        // Nothing the renderer wrote is lost.
        assert_eq!(
            document.get(SDK_TYPE_KEY).and_then(Value::as_str),
            Some("ResourceNotFoundException"),
            "got {document:?}"
        );
    }

    /// A failure whose shape carries only its text gets no `Details` member at all.
    #[test]
    fn a_failure_with_no_modelled_detail_carries_no_details_member() {
        let answer = with_response_envelope(answered(
            501,
            r#"{"__type":"NotImplemented","message":"not written yet"}"#,
            Some("NotImplemented"),
        ));
        let document = document(&answer);
        assert!(
            document.get(DOCUMENT_DETAILS_KEY).is_none(),
            "got {document:?}"
        );
    }

    /// A success is not a failure: its body is byte-identical and it still names its
    /// request.
    #[test]
    fn a_success_keeps_its_body_and_names_its_request() {
        let body = r#"{"arn":"arn"}"#;
        let answer = with_response_envelope(answered(200, body, None));
        assert_eq!(
            answer.body().bytes().expect("bytes"),
            body.as_bytes(),
            "a success body must be byte-identical"
        );
        assert!(
            answer.headers().get(REQUEST_ID_HEADER).is_some(),
            "a success names the request it answers too"
        );
    }

    /// An id a layer below already named the request by is the one the document states.
    #[test]
    fn an_already_named_request_keeps_its_id() {
        let mut resp = answered(
            400,
            r#"{"__type":"ValidationException","message":"no"}"#,
            Some("ValidationException"),
        );
        resp.headers_mut().insert(REQUEST_ID_HEADER, "already-named");
        let answer = with_response_envelope(resp);
        assert_eq!(
            document(&answer)
                .get(DOCUMENT_REQUEST_ID_KEY)
                .and_then(Value::as_str),
            Some("already-named"),
            "the id already on the response is the one stated"
        );
    }
}
