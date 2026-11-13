// SPDX-License-Identifier: CC0-1.0

//! How this service reports a refusal ANOTHER service answered.
//!
//! Part of this service's input is a REFERENCE into another service's store — an
//! `@arnReference` member — so the call that judges such a value is made downstream, and
//! the refusal it comes back with belongs to that other service rather than to this one.
//! BackupSearch does not re-word it: it reports the downstream reason exactly as the other
//! service stated it, inside the envelope the SDK layer that relayed the call states around
//! a failed call, so the caller can see WHICH service refused, with what HTTP status, and
//! under which request id — detail the modelled exception of this service carries nowhere
//! else (its other members name only the reference that was submitted).
//!
//! The envelope is three facts about the relayed call and nothing more: the service it was
//! made to, the status that service answered it with, and that call's own request id,
//! followed by how many attempts it took ([`relayed_refusal_message`]). A client reads the
//! service and the status back out of the text to attribute the failure and to decide
//! whether anything is worth retrying, so both are taken from the service that actually
//! answered rather than restated as this service's own.

/// A service this one relays a call to, as a relayed refusal names it: the name it is
/// reported under and the HTTP status it answered the relayed call with.
///
/// The two travel PAIRED, because they are the two values a client parses back out of a
/// relayed message — the service that refused and the status it refused with — and a
/// message that pairs one service's name with another's status would misattribute the
/// failure.
#[derive(Debug, Clone, Copy)]
pub struct RelayedService {
    /// The service's name, as the SDK layer that relayed the call reports it.
    pub name: &'static str,
    /// The HTTP status that service answered the relayed call with.
    pub status: u16,
}

/// How many attempts the relayed call is reported to have taken.
///
/// A refusal of the caller's OWN input is not retried — there is nothing about it a second
/// attempt could change — so the call that came back refused was the first and only one.
const RELAYED_ATTEMPTS: u32 = 1;

/// `reason`, the message the relayed-to service itself gave, as this service reports it to
/// its caller: that reason verbatim, then the relay envelope naming the call it came from.
///
/// The envelope is rendered by the workspace's one writer of it
/// (`service_common::relayed_refusal`), so this service's relayed refusals carry the same
/// form as every other service's. The request id in it is the id of the RELAYED call:
/// minted there, per call, so it is indistinguishable from the id any other response of
/// this emulator reports and two refusals never share one.
pub fn relayed_refusal_message(reason: &str, relayed: RelayedService) -> String {
    let RelayedService { name, status } = relayed;
    ::service_common::relayed_refusal::relayed_refusal_message(
        reason,
        ::service_common::relayed_refusal::RelayedCall {
            service: name,
            status,
            attempts: RELAYED_ATTEMPTS,
        },
    )
}

/// KMS, as a refusal this service relayed from it names that service.
///
/// The status is KMS's OWN, not a number chosen here: that service refuses a key
/// identifier that resolves to no key of the endpoint with its modelled
/// `NotFoundException` ([`KMS_KEY_NOT_FOUND_STATUS`]) and answers every client error of its
/// protocol with that protocol's client status, which is the mapping its own endpoint
/// applies ([`service_com_amazonaws_kms_trentservice::scaffolded::wire_status`]). So the
/// status a client reads out of a relayed message is the status the same call to KMS would
/// have answered the client with directly.
pub fn relayed_kms() -> RelayedService {
    RelayedService {
        name: KMS_RELAY_NAME,
        status: ::service_com_amazonaws_kms_trentservice::scaffolded::wire_status(
            KMS_KEY_NOT_FOUND_STATUS,
        ),
    }
}

/// The name a relayed refusal states KMS under: the service's name as the SDK layer
/// relaying the call spells it, which is neither its endpoint prefix (`kms`) nor the casing
/// of its `sdkId` (`KMS`).
const KMS_RELAY_NAME: &str = "Kms";

/// The status KMS declares for a key identifier that names no key: the `@httpError` of its
/// `NotFoundException`
/// (`smithy_query(query="shape",shape="NotFoundException",model_path="services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json")`),
/// which [`relayed_kms`] puts through that service's own wire-status mapping to get the
/// status its endpoint answers with.
const KMS_KEY_NOT_FOUND_STATUS: u16 = 404;
