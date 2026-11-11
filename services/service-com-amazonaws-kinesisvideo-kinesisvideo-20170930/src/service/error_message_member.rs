// SPDX-License-Identifier: CC0-1.0

//! Which member of an error body this service reports a failure's text in: `Message`.
//!
//! # The rule
//!
//! Every error shape this model declares carries exactly one member, `Message`, targeting
//! `ErrorMessage`, and none of them carries a `@jsonName`
//! (`smithy_query(query="shape",shape="AccessDeniedException,AccountChannelLimitExceededException,AccountStreamLimitExceededException,ClientLimitExceededException,DeviceStreamLimitExceededException,InvalidArgumentException,InvalidDeviceException,InvalidResourceFormatException,NoDataRetentionException,NotAuthorizedException,ResourceInUseException,ResourceNotFoundException,StreamEdgeConfigurationNotFoundException,TagsPerResourceExceededLimitException,VersionMismatchException",model_path="services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/smithy-model/com-amazonaws-kinesisvideo-kinesisvideo-20170930.json")`),
//! and a restJson1 member with no `@jsonName` goes on the wire under its Smithy name. So
//! `Message` is where one of this service's errors carries its text, whatever produced it:
//!
//! ```json
//! HTTP/1.1 400 Bad Request
//! content-type: application/json
//! x-amzn-errortype: InvalidArgumentException
//!
//! {"__type":"InvalidArgumentException","Message":"1 validation error detected: …"}
//! ```
//!
//! An error a HANDLER returns is serialized from the model's own shape, so it already
//! carries that member. The body the FRAMEWORK renders for a request it refused before any
//! handler ran does not: that body is built from a synthetic shape whose own member is
//! spelled `message`, and the JSON error renderer writes that spelling even when the
//! service names one of its MODELLED error shapes as the code. A client — or an SDK
//! deserializing the modelled exception the code names — then finds no `Message` member at
//! all and surfaces the refusal with no text, though its status and its code are right.
//!
//! This layer therefore reports that text in `Message` on the error bodies this service
//! answers that way, wherever it was produced. WHICH of them those are is
//! [`super::refusal_envelope`]'s: a refusal of an operation the stream stack fronts stays in
//! the generic validation envelope, code, header and `message` member together, and only the
//! other bodies are completed here — which is also where that layer restates the modelled
//! shape a body names before this renaming reads it.
//!
//! The renaming itself is `service-common`'s, so this file states only the two member names
//! and the shapes the rule covers; the status, the `__type` discriminator, the
//! `x-amzn-errortype` header, every other member and the whole success path stay with the
//! renderer that owns them. A failure whose class this model does not declare — the
//! scaffold's own 501 — keeps the member the framework wrote it in, because the rename is
//! keyed on the modelled shapes.
//!
//! The structural home for the rule is the framework's error renderer, which writes the
//! synthetic shape's `message` for every service; that renderer is in the agent-immutable
//! `vendor/` tree, so each service states it at the one seam that can see a whole rendered
//! response — an operation wrapper. A failure answered before any operation is served (a
//! request naming no modelled operation, a transport-level failure, a pre-dispatch
//! authentication refusal) is therefore not reached from here.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Service,
};

/// The body member this service's errors carry their text in — the member name every error
/// shape in the model declares; see the module header for the `smithy_query` call it comes
/// from.
pub const MODELLED_MESSAGE_MEMBER: &str = "Message";

/// The body member the FRAMEWORK's own refusal renderer writes that text in: the member of
/// the synthetic shape it builds a pre-handler refusal from.
const RENDERED_MESSAGE_MEMBER: &str = "message";

/// Every error shape this model declares, by the wire code (`__type`) it renders under —
/// the same `smithy_query` call the module header cites, which is where this list comes
/// from and the only thing that may extend it.
///
/// The rename is keyed on THIS set rather than on the status, so a failure whose class this
/// service does not model keeps the member the framework wrote its text in.
const MODELLED_ERROR_SHAPES: [&str; 15] = [
    "AccessDeniedException",
    "AccountChannelLimitExceededException",
    "AccountStreamLimitExceededException",
    "ClientLimitExceededException",
    "DeviceStreamLimitExceededException",
    "InvalidArgumentException",
    "InvalidDeviceException",
    "InvalidResourceFormatException",
    "NoDataRetentionException",
    "NotAuthorizedException",
    "ResourceInUseException",
    "ResourceNotFoundException",
    "StreamEdgeConfigurationNotFoundException",
    "TagsPerResourceExceededLimitException",
    "VersionMismatchException",
];

/// `resp` as this service answers it: a modelled error body whose text is in its `Message`
/// member, through the one renaming `service-common` states for every service whose wire
/// member differs from the one a layer wrote.
pub fn with_modelled_message_member(resp: RtResponse) -> RtResponse {
    service_common::error_detail_member::with_renamed_detail_member(
        resp,
        &MODELLED_ERROR_SHAPES,
        RENDERED_MESSAGE_MEMBER,
        MODELLED_MESSAGE_MEMBER,
    )
}

/// The operation `definition`'s generated routing table names for this request, if any.
///
/// The ONE resolution in this module: [`routes`] (the condition under which the service
/// claims a request for the wrapper) and [`ErrorMessageMemberOperation::run`] (the
/// operation it then delegates to) are both expressed through it, so the claim and the
/// delegation target cannot drift apart.
fn routed<'s>(
    definition: &'s dyn ErasedServiceDefinition,
    req: &RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = definition.protocol().routing_key(req).ok()?;
    definition.routing_table().get(key)
}

/// Whether the generated routing table can serve this request — the condition under which
/// this service claims it for [`ErrorMessageMemberOperation`].
///
/// A request the table cannot serve is declined rather than claimed, so the router answers
/// it as the unroutable request it is instead of handing it to a wrapper that would have to
/// guess.
pub fn routes(definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> bool {
    routed(definition, req).is_some()
}

/// Serves the modelled operation the routing table names for the request, exactly as that
/// table would, then answers whatever it produced in the envelope this service answers that
/// operation's refusals in ([`super::refusal_envelope`]).
///
/// WHY a wrapper and not [`Service::override_refusal`]: that seam chooses the CODE and the
/// WORDS of a pre-handler refusal, not the body member they are serialized into, nor the
/// header the shape is named in — the framework writes a framework-rendered error's text into
/// its own synthetic shape's `message`, and names the shape bare. A wrapper sees the whole
/// response, and it sees WHICH operation answered, which is what the envelope is a property
/// of.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`] — a wrapper held on the service cannot borrow from the entry it
/// is held beside, which is why the key is resolved here rather than carried in. The
/// framework therefore owns the whole deserialize -> validate -> handle -> serialize walk
/// and every status, code and word it answers with; this wrapper owns only which member the
/// text is in and how the shape is named.
#[derive(Debug, Clone, Copy)]
pub struct ErrorMessageMemberOperation;

impl ErasedOperation for ErrorMessageMemberOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read out of the definition BEFORE the await: `&dyn ErasedServiceDefinition` is
            // not `Sync`, so holding one across the await would make this future non-`Send`.
            let operation = svc
                .definition()
                .protocol()
                .routing_key(req)
                .unwrap_or_default()
                .to_string();
            match routed(svc.definition(), req) {
                Some(op) => super::refusal_envelope::with_refusal_envelope(
                    op.run(svc, req, ctx).await,
                    &operation,
                ),
                // Unreachable through `Service::override_operation`, which claims a request
                // only when the routing table holds its key ([`routes`]); answered as the
                // framework answers an unroutable request rather than guessed at.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The MODELLED operation the routing key names decides which schemes sign a
        // request, and it is resolved from the routing table rather than from the operation
        // the service handed back, so this wrapper cannot weaken them. `None` is what the
        // router falls back to only when the model names none.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;
    use serde_json::Value;

    /// A response at `status` carrying `body`.
    fn response(status: u16, body: &str) -> RtResponse {
        RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    /// The body of `resp` as JSON.
    fn decoded(resp: &RtResponse) -> Value {
        serde_json::from_slice(resp.body().bytes().expect("an in-memory body"))
            .expect("the body is JSON")
    }

    /// RULE: one row per class of error body this service can answer with — a refusal the
    /// framework rendered under each of the codes the service names, and a modelled error a
    /// handler serialized — so the member is a property of the error envelope rather than of
    /// one recorded body.
    #[test]
    fn every_modelled_error_body_reports_its_text_in_the_message_member() {
        for code in MODELLED_ERROR_SHAPES {
            let text = format!("{code} states its own sentence");
            let body = format!(r#"{{"__type":"{code}","message":"{text}"}}"#);
            let answered = with_modelled_message_member(response(400, &body));
            let document = decoded(&answered);
            assert_eq!(
                document.get(MODELLED_MESSAGE_MEMBER).and_then(Value::as_str),
                Some(text.as_str()),
                "case {code:?}: expected the text in {MODELLED_MESSAGE_MEMBER}, got {document}"
            );
            assert!(
                document.get(RENDERED_MESSAGE_MEMBER).is_none(),
                "case {code:?}: the text must be carried under ONE member, got {document}"
            );
            assert_eq!(
                document.get("__type").and_then(Value::as_str),
                Some(code),
                "case {code:?}: the discriminator must survive the rename, got {document}"
            );
        }
    }

    /// RULE: a body this rule does not own is answered byte for byte as its renderer wrote
    /// it — a success, a failure whose class the model does not declare, and a body that
    /// already carries the modelled member.
    #[test]
    fn a_body_this_rule_does_not_own_is_answered_unchanged() {
        // (case, status, the body)
        let rows: &[(&str, u16, &str)] = &[
            (
                "a success whose own members are not an error's",
                200,
                r#"{"StreamInfo":{"Status":"ACTIVE","Version":"aBcD"}}"#,
            ),
            (
                "the scaffold's 501, a class this model does not declare",
                501,
                r#"{"__type":"NotImplemented","message":"not implemented in this emulator"}"#,
            ),
            (
                "a modelled error a handler already serialized",
                404,
                r#"{"__type":"ResourceNotFoundException","Message":"The requested stream is not found or not active."}"#,
            ),
        ];

        for (case, status, body) in rows {
            let answered = with_modelled_message_member(response(*status, body));
            assert_eq!(
                answered.body().bytes().expect("an in-memory body"),
                body.as_bytes(),
                "case {case:?}: expected the body unchanged"
            );
        }
    }
}
