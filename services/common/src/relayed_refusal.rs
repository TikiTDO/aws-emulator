// SPDX-License-Identifier: CC0-1.0

//! How a service reports a refusal ANOTHER service answered.
//!
//! An operation whose input names a resource of a plane this service does not own
//! resolves that reference by CALLING the service that owns it, so a refusal it comes
//! back with belongs to the called service rather than to the one the client addressed.
//! The calling service does not re-word it: the downstream reason travels verbatim
//! inside the envelope the SDK layer that relayed the call states around a failed call,
//! so the caller can see WHICH service refused, with what HTTP status and under which
//! request id — detail the calling service's own modelled exception carries nowhere else.
//!
//! The envelope is four facts about the relayed call and nothing more: the service it was
//! made to, the status that service answered it with, that call's own request id, and how
//! many attempts it took ([`relay_envelope`]). The service name and the status come from
//! the service that actually answered, because a message pairing one service's name with
//! another's status misattributes the failure; the request id belongs to the relayed CALL,
//! so it is minted here, per call, rather than derived from anything in the request.
//!
//! This is the workspace's ONE rendering of that envelope, so a client parsing the service,
//! the status or the request id back out of a message reads one form whichever emulated
//! service relayed the call.

use crate::query_error_envelope::new_request_id;

/// The relayed call a refusal is reported from: the service the call was made to, the
/// HTTP status that service answered it with, and how many attempts reaching it took.
///
/// The three travel TOGETHER because they are what a client parses back out of a relayed
/// message — who refused, with what status, and whether anything was retried — and a
/// message that mixes two calls' facts attributes the failure to the wrong service.
#[derive(Debug, Clone, Copy)]
pub struct RelayedCall<'a> {
    /// The called service's name, as the SDK layer that relayed the call reports it,
    /// which is neither its endpoint prefix nor the casing of its `sdkId`.
    pub service: &'a str,
    /// The HTTP status that service answered the relayed call with.
    pub status: u16,
    /// How many attempts the relayed call took.
    pub attempts: u32,
}

impl<'a> RelayedCall<'a> {
    /// A relayed call that was made exactly ONCE.
    ///
    /// A refusal of the caller's own input is not retried — there is nothing about it a
    /// second attempt could change — so the call that came back refused was the first and
    /// only one. A caller that does retry states its own count on [`RelayedCall`].
    #[must_use]
    pub fn once(service: &'a str, status: u16) -> Self {
        Self {
            service,
            status,
            attempts: 1,
        }
    }
}

/// The envelope a relayed refusal is reported inside, including the space that separates it
/// from the reason it follows — so a caller that carries a downstream reason through
/// APPENDS this to it.
#[must_use]
pub fn relay_envelope(call: RelayedCall<'_>) -> String {
    let RelayedCall {
        service,
        status,
        attempts,
    } = call;
    format!(
        " (Service: {service}, Status Code: {status}, Request ID: {}) \
         (SDK Attempt Count: {attempts})",
        new_request_id()
    )
}

/// `reason`, the message the called service itself gave, as the calling service reports it
/// to its caller: that reason verbatim, then the envelope naming the call it came from
/// ([`relay_envelope`]).
#[must_use]
pub fn relayed_refusal_message(reason: &str, call: RelayedCall<'_>) -> String {
    format!("{reason}{}", relay_envelope(call))
}

#[cfg(test)]
mod tests {
    use super::{relay_envelope, relayed_refusal_message, RelayedCall};
    use crate::query_error_envelope::is_request_id;

    /// The envelope states the relayed call's own facts, and the reason it follows is
    /// carried through untouched.
    #[test]
    fn a_relayed_refusal_states_the_reason_then_the_call_it_came_from() {
        let rows = [
            ("Knowledge Base does not exist.", "CloverKnowledge", 404u16, 1u32),
            ("key does not exist", "Kms", 400, 1),
            ("certificate authority not found", "EksCertificateAuthority", 404, 3),
        ];
        for (reason, service, status, attempts) in rows {
            let call = RelayedCall {
                service,
                status,
                attempts,
            };
            let message = relayed_refusal_message(reason, call);
            let head = format!("{reason} (Service: {service}, Status Code: {status}, Request ID: ");
            let tail = format!(") (SDK Attempt Count: {attempts})");
            assert!(
                message.starts_with(&head),
                "case {reason:?}: expected the message to start {head:?}, got {message:?}"
            );
            assert!(
                message.ends_with(&tail),
                "case {reason:?}: expected the message to end {tail:?}, got {message:?}"
            );
            let request_id = message
                .strip_prefix(&head)
                .and_then(|rest| rest.strip_suffix(&tail))
                .unwrap_or_default();
            assert!(
                is_request_id(request_id),
                "case {reason:?}: expected a request id, got {request_id:?}"
            );
        }
    }

    /// The request id belongs to the relayed CALL, so two refusals never report one id.
    #[test]
    fn each_relayed_call_reports_its_own_request_id() {
        let call = RelayedCall::once("CloverKnowledge", 404);
        let first = relay_envelope(call);
        let second = relay_envelope(call);
        assert_ne!(
            first, second,
            "case \"two relayed calls\": expected each call its own request id, got {first:?} \
             twice"
        );
        assert_eq!(
            call.attempts, 1,
            "case \"a call made once\": expected one attempt, got {}",
            call.attempts
        );
    }
}
