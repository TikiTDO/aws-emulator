// SPDX-License-Identifier: CC0-1.0

//! The AMAZON CONNECT INSTANCE GATE: whether the `connectInstanceId` a request names is
//! one the account's Amazon Connect instance registry holds, and what the answer is when
//! it is not.
//!
//! # The rule
//!
//! Amazon Connect owns the instance registry — its `CreateInstance` is the only call that
//! mints an instance id and its `DeleteInstance` the only one that drops it — so an
//! operation of another service that requires an instance resolves the id against THAT
//! service's own store rather than against a registry of its own: an instance created or
//! deleted over the Connect API is a target for it, or not, with no propagation step.
//!
//! Reading that registry has THREE outcomes, and only two of them are refusals:
//!
//! * the account holds an instance under the id — the gate passes;
//! * the account holds none — a REQUEST fault ([`ConnectInstanceRefusal::NoSuchInstance`]),
//!   carrying [`INSTANCE_NOT_USABLE_MESSAGE`] where the caller's own model words that class
//!   as this one;
//! * the registry could not be read at all — a SERVER fault
//!   ([`ConnectInstanceRefusal::RegistryUnavailable`]), carrying
//!   [`REGISTRY_UNAVAILABLE_MESSAGE`]. A registry this emulator cannot read answers the
//!   existence question not at all, and is never a reason to treat the instance as absent.
//!
//! [`connect_instance_refusal`] is that classification, stated once. The REFUSAL each
//! caller answers with is NOT here, because it is the caller's own model's: the first
//! ConnectCampaigns API version answers the registry outcome with its modelled
//! `ValidationException` (HTTP 400) and the second with its modelled
//! `ResourceNotFoundException` (HTTP 404), and an operation whose own resource IS the
//! instance's configuration words it after that resource instead. Each maps the outcome
//! above to the error its own model and recorded wire name, and this module decides only
//! WHICH outcome the registry gave.
//!
//! # Where the read itself lives
//!
//! This crate holds no service's store and depends on no service crate, so the READ is the
//! registry owner's: `registry_holds_instance` in
//! `service-com-amazonaws-connect-amazonconnectservice`'s `state`, the one function that
//! reaches Connect's own store through the shared engine and answers `Some(held)` or `None`
//! for a store it cannot reach. A caller hands what that read answered to
//! [`connect_instance_refusal`] and gets the outcome back; the registry's lock is taken and
//! released inside that read, with no guard of the caller's own store held, which is what
//! keeps the cross-service read one-directional and a lock cycle between the two stores
//! unreachable.
//!
//! This rule is CALLED by
//! `service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice` (`CreateCampaign`
//! and `StartInstanceOnboardingJob`) and by
//! `service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2`
//! (`CreateCampaign`, `StartInstanceOnboardingJob` and the instance-integration
//! operations), each through the one helper its own `state` exposes.

/// What a service answers when the Amazon Connect instance a request names is not one it
/// can be used with: the account's registry holds no instance under that id, or the
/// instance it holds is not enabled for the feature.
///
/// A FIXED sentence — it names no instance and carries nothing from the request, which is
/// what the recorded responses of this class carry — so every refusal of the class reads
/// the same and the response is deterministic.
pub const INSTANCE_NOT_USABLE_MESSAGE: &str =
    "Connect Instance does not exist or is not enabled to use Campaigns.";

/// What a service answers when the Amazon Connect instance registry — another service's
/// store, reached through the shared engine — cannot be read at all, so the instance
/// question has no answer here.
pub const REGISTRY_UNAVAILABLE_MESSAGE: &str =
    "the Amazon Connect instance registry is unavailable";

/// Why the Amazon Connect instance a request names is not usable, as
/// [`connect_instance_refusal`] reports it.
///
/// Two outcomes, because they are answered differently: an id the account's registry does
/// not hold is a request fault, while a registry that cannot be read at all is a server
/// fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectInstanceRefusal {
    /// The account holds no Amazon Connect instance under the id the request names, so
    /// there is nothing the caller could be used with: the caller's own modelled
    /// request-fault error, carrying the text its recorded wire words this class as —
    /// [`INSTANCE_NOT_USABLE_MESSAGE`] where that is the one the operation answers.
    NoSuchInstance,
    /// The registry could not be read, so the existence question has no answer: the
    /// caller's own modelled server error, carrying [`REGISTRY_UNAVAILABLE_MESSAGE`].
    RegistryUnavailable,
}

/// The gate, over what the registry read answered: `None` when the account holds the
/// instance, and otherwise which refusal the caller owes.
///
/// `registry_holds_instance` is what the registry owner's read answered — `Some(true)` for
/// an id the account holds, `Some(false)` for one it does not, and `None` for a registry
/// that could not be read at all (see this module's header for that read).
pub fn connect_instance_refusal(
    registry_holds_instance: Option<bool>,
) -> Option<ConnectInstanceRefusal> {
    match registry_holds_instance {
        Some(true) => None,
        Some(false) => Some(ConnectInstanceRefusal::NoSuchInstance),
        None => Some(ConnectInstanceRefusal::RegistryUnavailable),
    }
}
