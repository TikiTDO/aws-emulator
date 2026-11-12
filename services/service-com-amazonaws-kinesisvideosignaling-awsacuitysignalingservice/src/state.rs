// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice service.
//!
//! The service is the signaling *data* plane: a channel is created by the Kinesis Video
//! control plane (`CreateSignalingChannel`), and neither operation here creates one. So the
//! state this crate owns is a **signaling-channel registry** keyed by channel ARN, plus
//! what each channel's data plane holds: the TURN credential set `GetIceServerConfig`
//! issues, the Alexa SDP offers `SendAlexaOfferToMaster` delivers, and the peers attached
//! to the channel's own WebSocket.
//!
//! A channel record is seeded by whoever creates the channel: the Kinesis Video control
//! plane's `CreateSignalingChannel` calls [`register_channel_of`] (and its
//! `DeleteSignalingChannel` calls [`forget_channel_of`]) on this store, which is the one
//! registry both planes read, so a channel the control plane reports as existing is a
//! channel this data plane serves. An ARN that is not in the registry is a
//! `ResourceNotFoundException`, never a success.
//!
//! Beside the registry the store holds the channels' live WebSocket data plane: the socket
//! every channel is reached at ([`wss_endpoint`]) and, per channel, the peers attached to
//! it ([`session`]).

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

/// Whether IAM lets a request to this data plane perform the action it asks for.
pub mod authorization;
/// The peers attached to one channel, and the frames routed between them.
pub mod session;
/// The address every channel's WebSocket data plane is reached at.
pub mod wss_endpoint;

pub use authorization::authorization_refusal;
pub use session::{Attachment, ChannelSession, PeerConnection};
pub use wss_endpoint::{ensure_wss_endpoint, SignalingEndpoint, WSS_SCHEME};

/// Lower bound of the modelled `@range` on `IceServer$Ttl`
/// (`smithy_query(query="shape", shape="IceServer")`: range 30..86400).
pub const MIN_ICE_CREDENTIAL_TTL_SECS: i32 = 30;

/// Upper bound of the modelled `@range` on `IceServer$Ttl` (see
/// [`MIN_ICE_CREDENTIAL_TTL_SECS`]).
pub const MAX_ICE_CREDENTIAL_TTL_SECS: i32 = 86400;

/// Lifetime of an issued TURN credential set when the channel record does not state one.
///
/// Not a documented AWS duration — a chosen default, inside the modelled
/// 30..86400 range. A channel record may carry its own
/// [`ice_credential_ttl_secs`](SignalingChannel::ice_credential_ttl_secs).
pub const DEFAULT_ICE_CREDENTIAL_TTL_SECS: i32 = 300;

/// How many Alexa SDP offers one channel's log keeps.
///
/// Not an AWS quota — a chosen bound. Every `SendAlexaOfferToMaster` call appends a
/// record holding the caller's `MessagePayload` (modelled up to 10000 characters), so
/// without a cap a caller looping that one route grows this process without limit. At the
/// cap the OLDEST offer is dropped: the log is a recent-delivery history, not an archive.
pub const MAX_OFFERS_PER_CHANNEL: usize = 128;

/// How many signaling channels the registry holds.
///
/// Not an AWS quota — a chosen bound. Channels are put here by the Kinesis Video control
/// plane's `CreateSignalingChannel`, which carries a ceiling of its own, and the registry
/// is bounded here too so one store cannot grow without end: past the cap a new channel is
/// refused rather than evicting a channel someone is using.
pub const MAX_CHANNELS: usize = 10_000;

/// Upper bound of the modelled `@length` on `ResourceARN`
/// (`smithy_query(query="shape", shape="ResourceARN")`: len 1..1024), the member both
/// operations take their `ChannelARN` from.
pub const MAX_CHANNEL_ARN_LEN: usize = 1024;

/// The first field of every ARN.
const ARN_FIRST_FIELD: &str = "arn";

/// Lower bound of the modelled `@range` on the control plane's
/// `SingleMasterConfiguration$MessageTtlSeconds` (`smithy_query(query="shape",
/// shape="SingleMasterConfiguration")` on the Kinesis Video model: range 5..120), the
/// member that states "the period of time (in seconds) a signaling channel retains
/// undelivered messages before they are discarded".
pub const MIN_MESSAGE_TTL_SECS: i32 = 5;

/// Upper bound of that same `@range`; see [`MIN_MESSAGE_TTL_SECS`].
pub const MAX_MESSAGE_TTL_SECS: i32 = 120;

/// The retention a channel record that states none keeps: "The default configuration for
/// the channel message's time to live is 60 seconds (1 minute)"
/// (`CreateSignalingChannel$SingleMasterConfiguration` docs).
pub const DEFAULT_MESSAGE_TTL_SECS: i32 = 60;

/// Per-service state for com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisvideosignalingAwsacuitysignalingserviceState {
    /// Every signaling channel this emulator knows, keyed by its ARN.
    pub channels: HashMap<String, SignalingChannel>,
    /// The socket every channel's WebSocket data plane is served on, opened on the first
    /// `WSS` endpoint resolution ([`ensure_wss_endpoint`]) and held for the life of the
    /// store, so an announced address never moves under a client.
    pub wss_endpoint: Option<SignalingEndpoint>,
}

/// Why a channel ARN in a request did not resolve to a channel record.
///
/// The two cases are distinct answers on the wire — a malformed ARN is the caller's
/// mistake (`InvalidArgumentException`, 400) while a well-formed ARN for a channel that
/// does not exist is `ResourceNotFoundException` (404) — so the registry reports which
/// one it was and lets each operation name its own modelled error. The not-found case
/// carries the channel NAME it parsed out of the ARN, which is the only part of the
/// request that refusal states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelLookupError {
    /// The ARN is not a Kinesis Video *signaling channel* ARN at all.
    MalformedArn,
    /// A well-formed channel ARN that no registered channel carries.
    NotFound {
        /// The channel-name segment of the ARN that did not resolve.
        channel_name: String,
    },
}

impl ComAmazonawsKinesisvideosignalingAwsacuitysignalingserviceState {
    /// Put `channel` in the registry, replacing any channel with the same ARN.
    ///
    /// `false` when the registry already holds [`MAX_CHANNELS`] other channels, in which
    /// case nothing is stored: a registration is refused rather than evicting a channel
    /// that callers are signaling over. `#[must_use]`, so a caller cannot drop the refusal
    /// and then be surprised by a 404 for the channel it believes it created.
    #[must_use]
    pub fn register_channel(&mut self, channel: SignalingChannel) -> bool {
        if self.channels.len() >= MAX_CHANNELS && !self.channels.contains_key(&channel.arn) {
            return false;
        }
        self.channels.insert(channel.arn.clone(), channel);
        true
    }

    /// The channel `arn` names, for a reporting read.
    pub fn resolve_channel(&self, arn: &str) -> Result<&SignalingChannel, ChannelLookupError> {
        let parsed = ChannelArn::parse_channel(arn).ok_or(ChannelLookupError::MalformedArn)?;
        self.channels
            .get(arn)
            .ok_or(ChannelLookupError::NotFound {
                channel_name: parsed.name,
            })
    }

    /// The channel `arn` names, for an operation that records what it did on the channel.
    pub fn resolve_channel_mut(
        &mut self,
        arn: &str,
    ) -> Result<&mut SignalingChannel, ChannelLookupError> {
        let parsed = ChannelArn::parse_channel(arn).ok_or(ChannelLookupError::MalformedArn)?;
        self.channels
            .get_mut(arn)
            .ok_or(ChannelLookupError::NotFound {
                channel_name: parsed.name,
            })
    }

    /// Drop the channel `arn` names, which is what the control plane deleting it does.
    ///
    /// Every peer attached to it goes with the record, so a viewer or master that was
    /// signaling over a deleted channel is forgotten and its socket task ends on its next
    /// frame.
    pub fn forget_channel(&mut self, arn: &str) -> bool {
        self.channels.remove(arn).is_some()
    }
}

/// What came of putting a channel in this emulator's signaling registry.
///
/// The two failures are different answers for the control plane to give: a registry at its
/// ceiling is a LIMIT, which the create's model declares an error for, while a missing
/// store is a capability this emulator does not have on that engine at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelRegistration {
    /// The channel is in the registry, and the data plane serves it.
    Registered,
    /// The registry already holds [`MAX_CHANNELS`] other channels, so nothing was stored:
    /// a registration is refused rather than evicting a channel callers are signaling over.
    AtCapacity,
    /// This emulator carries no store for the signaling service (it was mounted on an
    /// engine that has none), or the ARN is not a signaling-channel ARN — in neither case
    /// is there a registry entry to make.
    Unservable,
}

/// Put the channel `arn` names in THIS emulator's signaling registry, with the message
/// retention its control-plane record states — the seam the Kinesis Video control plane's
/// `CreateSignalingChannel` calls, so a created channel is one this data plane serves.
///
/// `message_ttl_secs` is held inside the modelled `MessageTtlSeconds` range, which is the
/// server-side bound on the redelivery window a caller's channel configuration chooses.
///
/// A channel already registered under that ARN keeps the peers attached to it: only its
/// retention is restated. Anything but [`ChannelRegistration::Registered`] leaves the
/// control plane to report the create it could not make servable rather than reporting a
/// channel the data plane cannot see.
pub fn register_channel_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    arn: &str,
    message_ttl_secs: i32,
) -> ChannelRegistration {
    let Some(state) = service_state(engine) else {
        return ChannelRegistration::Unservable;
    };
    let retention = message_ttl_secs.clamp(MIN_MESSAGE_TTL_SECS, MAX_MESSAGE_TTL_SECS);
    let mut store = state.lock();
    if let Some(held) = store.channels.get_mut(arn) {
        held.message_ttl_secs = retention;
        return ChannelRegistration::Registered;
    }
    let Some(mut channel) = SignalingChannel::from_arn(arn) else {
        return ChannelRegistration::Unservable;
    };
    channel.message_ttl_secs = retention;
    match store.register_channel(channel) {
        true => ChannelRegistration::Registered,
        false => ChannelRegistration::AtCapacity,
    }
}

/// Drop the channel `arn` names from this emulator's signaling registry — the seam the
/// control plane's `DeleteSignalingChannel` calls, so a deleted channel stops resolving on
/// the data plane.
///
/// `false` when there is no store to read or the registry held no such channel.
pub fn forget_channel_of<Eng: Send + Sync + 'static>(engine: &Eng, arn: &str) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    store.forget_channel(arn)
}

/// One signaling channel, as this data plane sees it.
#[derive(Debug)]
pub struct SignalingChannel {
    /// The channel's ARN — the registry key, and what both operations take as input.
    pub arn: String,
    /// The channel name, from the ARN's resource part.
    pub name: String,
    /// The region the channel lives in, from the ARN. The TURN relay the channel is
    /// served by is addressed in this region.
    pub region: String,
    /// The owning account id, from the ARN.
    pub account: String,
    /// The channel's creation-time id, from the ARN's trailing field: what distinguishes
    /// two channels that carried one name at different times.
    pub creation_id: String,
    /// The viewer client ids this channel accepts, when it restricts them. Empty means
    /// the channel accepts any client id (including none), which is the default.
    pub viewer_client_ids: BTreeSet<String>,
    /// How long an issued TURN credential set stays valid, in seconds.
    pub ice_credential_ttl_secs: i32,
    /// The TURN credential set this channel is currently serving, if one has been issued
    /// and is still re-servable.
    ///
    /// ONE per channel, not one per viewer: the credentials belong to the channel, so
    /// every viewer asking inside an issuance's lifetime is answered with that same
    /// issuance — which is what a relay honours and what recorded behaviour shows (clients
    /// asking concurrently share one credential).
    pub ice_credential: Option<IceServerRecord>,
    /// Every Alexa SDP offer delivered to this channel, oldest first.
    pub offers: Vec<AlexaOffer>,
    /// How long this channel retains a message it has not delivered, in seconds: its
    /// control-plane `SingleMasterConfiguration.MessageTtlSeconds`, which is the window an
    /// undelivered offer is redelivered over.
    pub message_ttl_secs: i32,
    /// The peers attached to this channel's WebSocket data plane right now, and the
    /// answers they owe.
    pub session: ChannelSession,
}

impl SignalingChannel {
    /// A channel whose identity is read out of `arn`; `None` when `arn` is not a
    /// well-formed Kinesis Video signaling-channel ARN.
    pub fn from_arn(arn: &str) -> Option<Self> {
        let parsed = ChannelArn::parse_channel(arn)?;
        Some(Self {
            arn: arn.to_string(),
            name: parsed.name,
            region: parsed.region,
            account: parsed.account,
            creation_id: parsed.creation_id,
            viewer_client_ids: BTreeSet::new(),
            ice_credential_ttl_secs: DEFAULT_ICE_CREDENTIAL_TTL_SECS,
            ice_credential: None,
            offers: Vec::new(),
            message_ttl_secs: DEFAULT_MESSAGE_TTL_SECS,
            session: ChannelSession::default(),
        })
    }

    /// The window an undelivered message of this channel is redelivered over: its
    /// `MessageTtlSeconds`, held inside the modelled range whatever the record says, so
    /// the wait a caller's configuration chooses is bounded server-side.
    #[must_use]
    pub fn message_ttl(&self) -> Duration {
        let seconds = self
            .message_ttl_secs
            .clamp(MIN_MESSAGE_TTL_SECS, MAX_MESSAGE_TTL_SECS);
        Duration::from_secs(seconds.unsigned_abs().into())
    }

    /// The credential lifetime this channel issues, held inside the modelled
    /// `IceServer$Ttl` range whatever the record says.
    pub fn ttl_secs(&self) -> i32 {
        self.ice_credential_ttl_secs
            .clamp(MIN_ICE_CREDENTIAL_TTL_SECS, MAX_ICE_CREDENTIAL_TTL_SECS)
    }

    /// The credential set this channel is serving that is still REUSABLE at `now`, if any.
    ///
    /// Keyed on nothing but the channel: a TURN credential set is the channel's, so a
    /// viewer asking inside the issuance's lifetime is answered with the issuance already
    /// made rather than with one of its own. That is what recorded behaviour shows —
    /// clients asking concurrently are answered with ONE shared credential — and it is
    /// what a relay enforces, since the credential it honours is the one that was issued.
    ///
    /// Reusable is narrower than unexpired: what goes on the wire is the lifetime the
    /// issuance has LEFT ([`IceServerRecord::reportable_ttl_secs`]), and the modelled
    /// `IceServer$Ttl` range starts at [`MIN_ICE_CREDENTIAL_TTL_SECS`], so an issuance
    /// with less than that left cannot be reported truthfully and is reissued instead of
    /// re-served. See [`IceServerRecord::is_reusable_at`].
    pub fn valid_credential(&self, now: SystemTime) -> Option<&IceServerRecord> {
        self.ice_credential
            .as_ref()
            .filter(|record| record.is_reusable_at(now))
    }

    /// Record `record` as this channel's credential set, replacing the one it was serving,
    /// and hand back the stored record.
    ///
    /// Bounded by construction: one issuance per channel, so a caller cannot make the
    /// record count its own.
    pub fn record_credential(&mut self, record: IceServerRecord) -> &IceServerRecord {
        self.ice_credential = Some(record);
        self.ice_credential
            .as_ref()
            .expect("the record just stored is present")
    }

    /// Append `offer` to this channel's offer log and hand back the stored record.
    ///
    /// Bounded at [`MAX_OFFERS_PER_CHANNEL`]: the oldest offers are dropped BEFORE the
    /// new one is stored, so repeated calls cannot grow this process without limit.
    pub fn record_offer(&mut self, offer: AlexaOffer) -> &AlexaOffer {
        // One pass, as in `record_credential`: everything above the cap is dropped
        // together, oldest first.
        let over_cap = (self.offers.len() + 1).saturating_sub(MAX_OFFERS_PER_CHANNEL);
        self.offers.drain(..over_cap);
        self.offers.push(offer);
        self.offers
            .last()
            .expect("the offer just pushed is present")
    }

    /// Every offer `sender_client_id` has sent to this channel, oldest first.
    pub fn offers_from<'a>(
        &'a self,
        sender_client_id: &'a str,
    ) -> impl Iterator<Item = &'a AlexaOffer> {
        self.offers
            .iter()
            .filter(move |offer| offer.sender_client_id == sender_client_id)
    }
}

/// One issuance of a TURN credential set, as `GetIceServerConfig` returns it.
///
/// `Debug` is written by hand so the minted `password` is never rendered: this record is
/// named in a `debug_assert!` message, and a secret the caller alone is meant to hold does
/// not belong in a panic message or a log line. Every other field is shown.
#[derive(Clone)]
pub struct IceServerRecord {
    /// The relay URIs the credentials are good for.
    pub uris: Vec<String>,
    /// The username the client logs in to the relay with.
    pub username: String,
    /// The password the client logs in to the relay with.
    pub password: String,
    /// How long, in seconds, this issuance stays valid.
    pub ttl_secs: i32,
    /// When it was issued.
    pub issued_at: SystemTime,
}

impl std::fmt::Debug for IceServerRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IceServerRecord")
            .field("uris", &self.uris)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("ttl_secs", &self.ttl_secs)
            .field("issued_at", &self.issued_at)
            .finish()
    }
}

impl IceServerRecord {
    /// The lifetime this issuance is honoured for: `ttl_secs` held inside the modelled
    /// `IceServer$Ttl` range, so the lifetime enforced here is the one reported on the
    /// wire and the two cannot diverge.
    pub fn effective_ttl_secs(&self) -> i32 {
        self.ttl_secs
            .clamp(MIN_ICE_CREDENTIAL_TTL_SECS, MAX_ICE_CREDENTIAL_TTL_SECS)
    }

    /// The instant this issuance stops being valid.
    pub fn expires_at(&self) -> SystemTime {
        self.issued_at + Duration::from_secs(self.effective_ttl_secs().unsigned_abs().into())
    }

    /// Whether the issuance is still valid at `now`.
    pub fn is_valid_at(&self, now: SystemTime) -> bool {
        now < self.expires_at()
    }

    /// How many seconds of validity the issuance has LEFT at `now`, rounded UP to the
    /// whole second a client would still be inside (so an issuance read microseconds
    /// after it was minted has its whole lifetime left, not one second less), and `0`
    /// once the clock has passed [`expires_at`](Self::expires_at).
    pub fn remaining_secs(&self, now: SystemTime) -> u64 {
        let left = self
            .expires_at()
            .duration_since(now)
            .unwrap_or(Duration::ZERO);
        left.as_secs() + u64::from(left.subsec_nanos() > 0)
    }

    /// The `Ttl` to report for this issuance at `now`: the lifetime it has LEFT, which is
    /// what the relay will still honour, held inside the modelled `IceServer$Ttl` range.
    ///
    /// Reporting the full lifetime again on every repeat call would tell a viewer at
    /// `issued_at + 290s` of a 300-second issuance that it is good for another 300
    /// seconds, while the registry stops honouring it in 10 — so what is reported is the
    /// remainder, and an issuance with less than [`MIN_ICE_CREDENTIAL_TTL_SECS`] left is
    /// not re-served at all ([`is_reusable_at`](Self::is_reusable_at)).
    pub fn reportable_ttl_secs(&self, now: SystemTime) -> i32 {
        // Clamping UP would overstate a remainder below the modelled minimum, which is
        // why an issuance that far gone is never re-served: the clamp is here so the
        // member cannot leave the modelled range, not as a way to round a dying issuance
        // back up. The gate is `is_reusable_at`, asserted so the invariant cannot decay
        // silently if that gate ever moves.
        debug_assert!(
            self.is_reusable_at(now),
            "only a reusable issuance is reported: {self:?} at {now:?}"
        );
        let remaining = i32::try_from(self.remaining_secs(now)).unwrap_or(i32::MAX);
        remaining.clamp(MIN_ICE_CREDENTIAL_TTL_SECS, MAX_ICE_CREDENTIAL_TTL_SECS)
    }

    /// Whether the issuance can be RE-SERVED at `now`: it still has at least the modelled
    /// minimum `Ttl` left, so the remainder reported on the wire is both truthful and
    /// inside `IceServer$Ttl`'s range.
    pub fn is_reusable_at(&self, now: SystemTime) -> bool {
        self.remaining_secs(now) >= MIN_ICE_CREDENTIAL_TTL_SECS.unsigned_abs().into()
    }
}

/// One Alexa SDP offer delivered to a channel's master peer.
#[derive(Debug, Clone)]
pub struct AlexaOffer {
    /// The client that sent the offer.
    pub sender_client_id: String,
    /// The base64-encoded SDP offer, as the sender supplied it.
    pub message_payload: String,
    /// When the offer was delivered.
    pub received_at: SystemTime,
    /// The SDP answer the master returned for this offer, when one was connected.
    pub answer: Option<String>,
}

/// The parts of a Kinesis Video resource ARN.
///
/// The shape is the modelled `ResourceARN` pattern
/// `^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`
/// (`smithy_query(query="shape", shape="ResourceARN")`), i.e.
/// `arn:aws:kinesisvideo:<region>:<account>:<type>/<name>/<creation id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelArn {
    /// The ARN's region field.
    pub region: String,
    /// The ARN's account-id field.
    pub account: String,
    /// The resource type — `channel` for a signaling channel, `stream` for a video stream.
    pub resource_type: String,
    /// The resource name.
    pub name: String,
    /// The channel's creation-time id (the ARN's trailing numeric field).
    pub creation_id: String,
}

impl ChannelArn {
    /// The resource type a signaling-channel ARN carries.
    pub const SIGNALING_CHANNEL_TYPE: &'static str = "channel";

    /// The ARN vendor (service namespace) a Kinesis Video resource ARN carries.
    const VENDOR: &'static str = "kinesisvideo";

    /// The partition this service's `ResourceARN` names LITERALLY
    /// (`^arn:aws:kinesisvideo:…`), where its Kinesis Video peers admit any `aws*` one.
    const PARTITION: &'static str = "aws";

    /// Read `arn` as a Kinesis Video resource ARN, of whatever resource type; `None`
    /// when it does not have the modelled shape.
    ///
    /// This is the modelled `ResourceARN` `@pattern` read field by field
    /// (`smithy_query(query="shape", shape="ResourceARN")`:
    /// `^arn:aws:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$`, len
    /// 1..1024) — including the `aws` partition this service names LITERALLY, where its
    /// Kinesis Video peers admit any `aws*` one.
    ///
    /// Modelled constraints are refused before a handler runs, so a handler normally gets
    /// a string this parses; it still answers `None` with its own modelled
    /// `InvalidArgumentException` rather than assuming the shape.
    pub fn parse(arn: &str) -> Option<Self> {
        if arn.len() > MAX_CHANNEL_ARN_LEN {
            return None;
        }
        // The six ARN fields: `splitn(6, …)` leaves the whole resource — colons included —
        // as the last one, so an ARN carrying a colon inside its resource is refused by
        // the accept-set below rather than silently re-split.
        let mut fields = arn.splitn(6, ':');
        if fields.next()? != ARN_FIRST_FIELD
            || fields.next()? != Self::PARTITION
            || fields.next()? != Self::VENDOR
        {
            return None;
        }
        let region = fields.next()?;
        let account = fields.next()?;
        let resource = fields.next()?;
        // Exactly three slash-separated resource parts: `splitn(4, …)` so a fourth part
        // is visible (and refused) without collecting the segments.
        let mut segments = resource.splitn(4, '/');
        let resource_type = segments.next()?;
        let name = segments.next()?;
        let creation_id = segments.next()?;
        if segments.next().is_some() {
            return None;
        }
        let well_formed = !region.is_empty()
            && region
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !account.is_empty()
            && account.bytes().all(|b| b.is_ascii_digit())
            && !resource_type.is_empty()
            && resource_type.bytes().all(|b| b.is_ascii_lowercase())
            && !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
            && !creation_id.is_empty()
            && creation_id.bytes().all(|b| b.is_ascii_digit());
        well_formed.then(|| Self {
            region: region.to_string(),
            account: account.to_string(),
            resource_type: resource_type.to_string(),
            name: name.to_string(),
            creation_id: creation_id.to_string(),
        })
    }

    /// Read `arn` as a *signaling channel* ARN: as [`parse`](Self::parse), and `None`
    /// for any other Kinesis Video resource type (a `stream` ARN, say).
    pub fn parse_channel(arn: &str) -> Option<Self> {
        let parsed = Self::parse(arn)?;
        (parsed.resource_type == Self::SIGNALING_CHANNEL_TYPE).then_some(parsed)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisvideosignalingAwsacuitysignalingserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisvideosignalingAwsacuitysignalingserviceState>)
}
