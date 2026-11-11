// SPDX-License-Identifier: CC0-1.0

//! The custom resource RESPONSE plane: the target a provider request publishes as its
//! `ResponseURL`.
//!
//! # Not a modelled operation
//!
//! A custom resource's provider answers CloudFormation by sending its response document to
//! the pre-signed `ResponseURL` the request carried — a plain HTTP `PUT` that names no
//! modelled operation, carries no `X-Amz-Target`, and presents no credential of its own, as
//! a pre-signed URL is followed. Nothing in the generated routing table can name it, so it
//! is reached through the two seams any emulated data plane uses:
//!
//! * [`crate::service::EmulatedService::can_handle`] also claims a request whose PATH is one
//!   of the URLs this service published
//!   ([`crate::state::provisioning::custom_resource::parse_response_path`]) — the path
//!   namespace a plane served at the emulator's own endpoint is addressed under
//!   ([`::service_common::data_plane_addressing`]), which no modelled route of this service
//!   can collide with;
//! * [`smithy_server_runtime::Service::override_routing_error`] serves it, and the router
//!   reaches that seam BEFORE authentication — which is what lets the published URL be
//!   followed unsigned.
//!
//! # What is served
//!
//! `PUT /_aws/cloudformation/<stack-id>|<logical-id>|<request-id>?<pre-signing query>`
//! records the provider's answer against the request it answers, which releases the wait the
//! resource is held at ([`crate::state::stabilising`]). The URL is single-purpose: it answers
//! ONE request of one resource of one stack, and a write that does not present the signature
//! this service published for that request is refused
//! ([`crate::state::ComAmazonawsCloudformationCloudformationState::awaited_response_url`]) —
//! the path alone would admit a target a caller assembled, and this seam is reached before
//! the request is authenticated, so the comparison is this service's own, exactly as a
//! pre-signed URL whose signature does not verify is refused rather than served.
//!
//! The write itself is otherwise accepted whether or not CloudFormation is still waiting for
//! that answer, exactly as a pre-signed `PUT` is — a provider answering a request a cancelled
//! or concluded operation no longer waits for is not told its own answer failed — and an
//! answer no request awaits is simply recorded against nothing.
//!
//! Every other method on this path space is refused: the published URL describes the one
//! write a provider makes, and answering a read on it would serve a document the URL grants
//! no read of.
//!
//! # The error envelope
//!
//! This plane is plain HTTPS rather than a modelled operation, so a refusal is answered in
//! its own small JSON envelope carrying the code and a reason — never in this service's
//! awsQuery shapes, which belong to the modelled operations. Every message is built from the
//! constants below and carries nothing from the request, so a caller cannot probe for the
//! targets it was not handed.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;

use crate::state::provider_responses::ResponseOutcome;
use crate::state::provisioning::custom_resource::{self, ProviderResponse, ResponseTarget};

/// The media type a refusal of this plane is stated in.
const ERROR_CONTENT_TYPE: &str = "application/json";

/// What this plane says to a method the published URL does not describe.
const METHOD_NOT_ALLOWED_MESSAGE: &str = "A custom resource response is sent with PUT";

/// What this plane says to a body that is not a custom resource response document, naming
/// the member a well-formed one must carry.
const MALFORMED_RESPONSE_MESSAGE: &str =
    "A custom resource response must be a JSON document carrying";

/// What this plane says to a write whose signature is not the one published for the target it
/// names: the answer a pre-signed URL whose signature does not verify gives. One sentence
/// carrying nothing from the request.
const SIGNATURE_MISMATCH_MESSAGE: &str =
    "The request signature does not match the signature of this response target";

/// What this plane says when the body could not be read at all. The router buffers every
/// request before dispatch, so a body it cannot hand back in memory is a failure of the
/// server, never a caller's malformed response.
const UNREADABLE_BODY_MESSAGE: &str = "The server could not read the response document";

// What this plane does with a response document larger than one a provider may send. "The
// total size of the response body can't exceed 4096 bytes" (custom resource response
// objects), and a document past that bound is not read at all: the write is still accepted
// — a pre-signed `PUT` is — and the answer recorded for it is the resource's FAILURE with
// the response-too-long reason (`ProviderResponse::too_long`). So nothing a caller sends
// sizes this store either: a document past the bound leaves one fixed reason behind and
// none of its own bytes.

/// What this plane says when this service's store is not reachable.
const STORE_UNAVAILABLE_MESSAGE: &str = "The CloudFormation store is unavailable";

/// Serve one write to a published `ResponseURL`, and say whether the answer CONCLUDED
/// anything: `true` is an answer recorded against the request this emulator was waiting for,
/// which is what makes the work that answer owes due ([`crate::state::provider_driver`]);
/// `false` is every other write, including a second one to a URL whose request has already
/// been answered.
///
/// Whether the target names a request this emulator is waiting for an answer to is the
/// store's judgement
/// ([`crate::state::ComAmazonawsCloudformationCloudformationState::record_provider_response`]):
/// this function reads the request, reads the answer out of its body, and hands that answer
/// to the store. The WRITE is accepted either way — the URL is the target a pre-signed `PUT`
/// goes to — so the only thing refused here is a write this plane cannot read as one.
pub fn serve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    region: &str,
    target: &ResponseTarget,
) -> (RtResponse, bool) {
    if req.method() != "PUT" {
        return (
            refusal(405, "MethodNotAllowed", METHOD_NOT_ALLOWED_MESSAGE.to_string()),
            false,
        );
    }
    // The URL authorises ONE write, and its signature is what says so: a target assembled by
    // a caller carries no signature this service published for it. A write about a request
    // this emulator is not waiting for an answer to is accepted and recorded against nothing
    // — a pre-signed `PUT` is not told the resource has moved on — so there is a published
    // signature to hold it to only where a request is still awaited.
    let Some(state) = crate::state::service_state(engine) else {
        return (
            refusal(500, "InternalFailure", STORE_UNAVAILABLE_MESSAGE.to_string()),
            false,
        );
    };
    let query = req.uri().split('?').nth(1).unwrap_or_default();
    if let Some(published) = state.lock().awaited_response_url(region, target) {
        let published = published.split('?').nth(1).unwrap_or_default().to_string();
        if custom_resource::presented_signature(query)
            != custom_resource::presented_signature(&published)
        {
            return (
                refusal(
                    403,
                    "SignatureDoesNotMatch",
                    SIGNATURE_MISMATCH_MESSAGE.to_string(),
                ),
                false,
            );
        }
    }
    let Some(body) = req.body().bytes() else {
        return (
            refusal(500, "InternalFailure", UNREADABLE_BODY_MESSAGE.to_string()),
            false,
        );
    };
    // A document past the size a response may carry is one CloudFormation does not read:
    // the answer it records is the resource's failure with the response-too-long reason,
    // which is what then fails the resource and rolls its stack back.
    let answered = if body.len() > custom_resource::MAX_RESPONSE_BODY_BYTES {
        ProviderResponse::too_long()
    } else {
        let Ok(document) = ::serde_json::from_slice::<::serde_json::Value>(body) else {
            return (malformed("a Status and a PhysicalResourceId"), false);
        };
        match ProviderResponse::read(&document) {
            Ok(answered) => answered,
            Err(member) => return (malformed(member), false),
        }
    };
    let outcome = state.lock().record_provider_response(region, target, answered);
    // The published URL is a WRITE TARGET the provider was handed: the write is accepted
    // whether or not CloudFormation is still waiting for it, exactly as a pre-signed `PUT`
    // is, so a provider answering a request a cancelled or concluded operation no longer
    // waits for is not told its own answer failed. Whether the answer CONCLUDES anything is
    // CloudFormation's own question, and that is what the outcome decides — and what the
    // caller of this function drives the work of.
    (recorded(), outcome == ResponseOutcome::Recorded)
}

/// The answer to a body that is not a well-formed response document, naming `member` — the
/// member of the protocol a provider must send, which is not something read off the request.
fn malformed(member: &str) -> RtResponse {
    refusal(
        400,
        "InvalidRequest",
        format!("{MALFORMED_RESPONSE_MESSAGE} {member}"),
    )
}

/// The answer to a response this plane recorded: the provider's answer is in hand, and there
/// is nothing to report back about it — the empty 200 a pre-signed `PUT` is answered with.
fn recorded() -> RtResponse {
    let mut resp = RtResponse::new(
        StatusCode::try_from(200u16).expect("200 is a valid status"),
        SdkBody::empty(),
    );
    resp.headers_mut().insert("content-length", "0");
    resp
}

/// One response-plane refusal: the status, the code in `x-amzn-errortype`, and a JSON body
/// carrying the reason.
fn refusal(status: u16, code: &str, message: String) -> RtResponse {
    let body = ::serde_json::json!({ "code": code, "message": message }).to_string();
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", ERROR_CONTENT_TYPE);
    resp.headers_mut().insert("x-amzn-errortype", code.to_string());
    resp
}
