// SPDX-License-Identifier: CC0-1.0

//! The Event API's DATA plane as STATE: the two sockets each `Api` is served on, the
//! realtime connections open on it, and the subscriptions those connections hold.
//!
//! An Event API is not reached by one request and one response. `Api$dns` publishes "an
//! HTTP and a real-time endpoint": a publisher POSTs an event to the HTTP one, and a
//! subscriber completes a WebSocket handshake against the real-time one and then exchanges
//! FRAMES over it until either side closes. So each endpoint needs a SOCKET, which the
//! core owns (`engine.listeners()`), and this module holds the handles those sockets' lives
//! hang off together with the records of what is open on them.
//!
//! Three invariants it keeps, so nothing else has to re-decide them:
//!
//! * ONE pair of listeners per `Api`, opened when the API is created and dropped when the
//!   API is — dropping a [`ResourceListener`] closes the port and severs every connection
//!   on it, which is what makes a deleted API stop answering. The addresses they landed on
//!   are what that API publishes as its `dns` records
//!   ([`super::event_api_dns`]), so the value a caller hands a WebSocket or HTTP client is
//!   an address that client can reach.
//! * a connection and an event IDENTIFIER are MINTED here, from this plane's own counters,
//!   so neither is ever a function of what a client sent and no id is reused.
//! * a subscription belongs to the CONNECTION that asked for it: closing a connection takes
//!   its subscriptions with it, which is what makes the subscription count a client reads
//!   fall back to zero when a subscriber goes away.
//!
//! The records live in a field of
//! [`super::ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState`] rather than a store of
//! their own, so the records that say which APIs and channel namespaces exist and the
//! sockets those APIs are reached at are read under ONE lock and cannot drift apart.

use std::collections::BTreeMap;
use std::sync::Arc;

use ::emulator_server::ResourceListener;
use ::tokio::sync::mpsc::Sender;

/// The two sockets one Event API is served on, and what is open on them.
struct ApiSockets {
    /// The socket the API's HTTP endpoint (`Api$dns`'s `HTTP` record) is served on.
    publish: Arc<ResourceListener>,
    /// The socket the API's real-time endpoint (`Api$dns`'s `REALTIME` record) is served
    /// on.
    realtime: Arc<ResourceListener>,
    /// The realtime connections open on this API, keyed by the connection id this plane
    /// minted for each.
    connections: BTreeMap<String, ConnectionRecord>,
    /// Whether a gauge sampler is running for this API
    /// ([`EventDataPlane::claim_gauge_sampler`]).
    gauge_sampling: bool,
    /// Which claim the running sampler holds, so a sampler that has stood down cannot
    /// release a claim a later one took.
    gauge_generation: u64,
    /// The running sampler's task, held so the task a connection started stays stoppable:
    /// an API whose records go takes its sampler with it.
    gauge_sampler: Option<::tokio::task::JoinHandle<()>>,
}

/// One open realtime connection: the channel its frames are written over, and the
/// subscriptions it holds.
struct ConnectionRecord {
    /// Where a frame pushed TO this connection is queued. The connection's own task owns
    /// the receiving half, so a delivery is a real write over that socket; a full queue
    /// means the subscriber is not reading and the frame is dropped rather than buffered
    /// without limit.
    frames: Sender<String>,
    /// The subscriptions this connection holds, keyed by the subscription id the client
    /// chose, each the channel it subscribed to.
    subscriptions: BTreeMap<String, String>,
}

/// How many frames may wait for one subscriber before a delivery to it is dropped.
///
/// Stated here because it is what stands between a publisher and the host's memory: a
/// subscriber that stops reading holds at most this many frames, whatever is published.
pub const CONNECTION_FRAME_DEPTH: usize = 64;

/// How many realtime connections one API's real-time endpoint accepts at once. A
/// connection past it is refused rather than queued, so a client opening sockets in a loop
/// cannot grow this store without limit.
///
/// It bounds BOTH real-time endpoints this service serves — an Event API's
/// ([`super::super::service::event_api::realtime`]) and a GraphQL API's
/// ([`super::graphql_data_plane`]) — because the bound is on what one socket of this
/// emulator admits, not on which kind of API published it.
pub const CONNECTIONS_PER_API: usize = 256;

/// How many subscriptions one realtime connection holds. A subscribe past it is refused,
/// which bounds the records one connection can create.
pub const SUBSCRIPTIONS_PER_CONNECTION: usize = 100;

/// The Event API data plane's records: the sockets each API is served on, the connections
/// open on them, and the counters this plane's ids are minted from.
#[derive(Default)]
pub struct EventDataPlane {
    /// The sockets each Event API is served on, keyed by `apiId`. Holding the handles is
    /// what keeps the ports open; removing the entry closes them.
    apis: BTreeMap<String, ApiSockets>,
    /// Drives [`Self::mint_connection_id`]: how many connections this plane has accepted.
    connection_seed: u64,
    /// Drives [`Self::mint_event_identifier`]: how many events this plane has accepted.
    event_seed: u64,
}

// By hand, because a `ResourceListener` is a live socket and carries no `Debug` of its own:
// what is diagnosable about the plane is which APIs hold sockets, where each landed and
// what is open on them, not a handle's internals.
impl std::fmt::Debug for EventDataPlane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let apis: BTreeMap<&String, (String, String, usize, usize)> = self
            .apis
            .iter()
            .map(|(api_id, sockets)| {
                (
                    api_id,
                    (
                        authority_of(&sockets.publish),
                        authority_of(&sockets.realtime),
                        sockets.connections.len(),
                        sockets
                            .connections
                            .values()
                            .map(|connection| connection.subscriptions.len())
                            .sum(),
                    ),
                )
            })
            .collect();
        formatter
            .debug_struct("EventDataPlane")
            .field("apis", &apis)
            .field("connection_seed", &self.connection_seed)
            .field("event_seed", &self.event_seed)
            .finish()
    }
}

/// The `host:port` a core listener is addressed by, written as a URL parses it — the ONE
/// reading of a listener's address (`service_common::data_plane_addressing`).
pub fn authority_of(listener: &ResourceListener) -> String {
    ::service_common::data_plane_addressing::listener_authority(listener)
}

/// Stop the gauge sampler of an API whose records have just gone.
///
/// A sampler reads the API's connection records, so one whose API is no longer held has
/// nothing left to observe: it is stopped with the records rather than left to discover
/// that on its next wake-up.
fn abort_sampler(sockets: ApiSockets) {
    if let Some(sampler) = sockets.gauge_sampler {
        sampler.abort();
    }
}

/// Why a subscribe was not recorded, which is what the realtime endpoint answers
/// `subscribe_error` with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscribeRefusal {
    /// The connection is not open on this API — it closed while the subscribe was in
    /// flight.
    NoConnection,
    /// The connection already holds a subscription under the id the client chose.
    DuplicateId,
    /// The connection already holds [`SUBSCRIPTIONS_PER_CONNECTION`] subscriptions.
    TooMany,
}

impl EventDataPlane {
    /// Remember the sockets `api_id` is served on, and answer the `host:port` each landed
    /// on — the authorities that API's `dns` records publish.
    ///
    /// Replaces any pair the API already held, so re-opening closes the old ports rather
    /// than leaving two sockets answering for one API.
    pub fn remember_listeners(
        &mut self,
        api_id: &str,
        publish: Arc<ResourceListener>,
        realtime: Arc<ResourceListener>,
    ) -> (String, String) {
        let authorities = (authority_of(&publish), authority_of(&realtime));
        if let Some(replaced) = self.apis.insert(
            api_id.to_string(),
            ApiSockets {
                publish,
                realtime,
                connections: BTreeMap::new(),
                gauge_sampling: false,
                gauge_generation: 0,
                gauge_sampler: None,
            },
        ) {
            abort_sampler(replaced);
        }
        authorities
    }

    /// The `host:port` pair (HTTP, then real-time) `api_id` is reached at, or `None` when
    /// this API holds no sockets.
    pub fn authorities(&self, api_id: &str) -> Option<(String, String)> {
        self.apis
            .get(api_id)
            .map(|sockets| (authority_of(&sockets.publish), authority_of(&sockets.realtime)))
    }

    /// Close `api_id`'s sockets and forget everything open on them — what an API being
    /// deleted does to its data plane. Dropping the handles severs every connection, and
    /// the task serving each one then ends.
    pub fn forget_api(&mut self, api_id: &str) {
        if let Some(forgotten) = self.apis.remove(api_id) {
            abort_sampler(forgotten);
        }
    }

    /// Claim the single GAUGE SAMPLER of `api_id`: the claim's GENERATION when the caller
    /// may start one, `None` when one already runs or this API holds no sockets.
    ///
    /// The two gauges this plane reports ([`Self::active_connections`],
    /// [`Self::active_subscriptions`]) measure quantities that HAVE a value at every
    /// instant, so they are OBSERVED on a cadence rather than only when they change: one
    /// sampler per API, claimed by the first connection opened on it and stood down once
    /// the API holds none ([`Self::gauge_sampling_wanted_or_release`]).
    pub fn claim_gauge_sampler(&mut self, api_id: &str) -> Option<u64> {
        let sockets = self.apis.get_mut(api_id)?;
        if sockets.gauge_sampling {
            return None;
        }
        sockets.gauge_sampling = true;
        sockets.gauge_generation = sockets.gauge_generation.wrapping_add(1);
        Some(sockets.gauge_generation)
    }

    /// Keep the running sampler's task handle, aborting the one it replaces — and only
    /// while `generation` is still the current claim AND that claim is still held, so a
    /// starter whose own task has already stood down installs nothing and aborts nothing.
    pub fn set_gauge_sampler(
        &mut self,
        api_id: &str,
        generation: u64,
        handle: ::tokio::task::JoinHandle<()>,
    ) {
        let Some(sockets) = self.apis.get_mut(api_id) else {
            handle.abort();
            return;
        };
        if !sockets.gauge_sampling || sockets.gauge_generation != generation {
            handle.abort();
            return;
        }
        if let Some(previous) = sockets.gauge_sampler.replace(handle) {
            previous.abort();
        }
    }

    /// Whether `api_id` still has gauges to sample — and, when it has not, RELEASE the
    /// claim `generation` holds.
    ///
    /// An API has gauges to sample while it holds a CONNECTION: the quantities are of the
    /// connections open on the API and the subscriptions they hold, so an API nothing is
    /// connected to has nothing to observe and publishes no reading at all rather than a
    /// run of zeros. Observing "nothing to sample" and standing down happen in ONE critical
    /// section, which is what makes the hand-over race-free: a handshake holds this same
    /// lock to record its connection BEFORE it tries to claim the sampler, so either this
    /// call sees that connection and the running sampler keeps it, or the handshake's claim
    /// finds the flag cleared and starts a sampler of its own.
    pub fn gauge_sampling_wanted_or_release(&mut self, api_id: &str, generation: u64) -> bool {
        let Some(sockets) = self.apis.get_mut(api_id) else {
            return false;
        };
        let wanted = !sockets.connections.is_empty();
        if !wanted && sockets.gauge_generation == generation {
            sockets.gauge_sampling = false;
        }
        wanted
    }

    /// Mint the next connection id: a UUID, which is the shape AppSync's own realtime
    /// endpoint identifies a connection and a subscription by.
    ///
    /// Minted from this plane's own counter, so an id is never reused and nothing a client
    /// sends chooses it.
    pub fn mint_connection_id(&mut self) -> String {
        self.connection_seed = self.connection_seed.wrapping_add(1);
        uuid_from(CONNECTION_ID_MIX_DOMAIN, self.connection_seed)
    }

    /// Mint the next event identifier — the `identifier` a publish answers for one event,
    /// a UUID as AppSync's own publish answers it.
    pub fn mint_event_identifier(&mut self) -> String {
        self.event_seed = self.event_seed.wrapping_add(1);
        uuid_from(EVENT_IDENTIFIER_MIX_DOMAIN, self.event_seed)
    }

    /// Record that `connection_id` is open on `api_id`, with `frames` as the queue a
    /// delivery to it is written through.
    ///
    /// `false` when the API holds no sockets (it was deleted) or already holds
    /// [`CONNECTIONS_PER_API`] connections, which is what refuses the handshake instead of
    /// admitting a connection this plane does not account for.
    pub fn open_connection(
        &mut self,
        api_id: &str,
        connection_id: String,
        frames: Sender<String>,
    ) -> bool {
        let Some(sockets) = self.apis.get_mut(api_id) else {
            return false;
        };
        if sockets.connections.len() >= CONNECTIONS_PER_API {
            return false;
        }
        sockets.connections.insert(
            connection_id,
            ConnectionRecord {
                frames,
                subscriptions: BTreeMap::new(),
            },
        );
        true
    }

    /// Forget `connection_id` and answer how many subscriptions went with it: a connection
    /// that is gone holds none, which is what makes the subscription count fall back when a
    /// subscriber goes away.
    pub fn close_connection(&mut self, api_id: &str, connection_id: &str) -> usize {
        let Some(sockets) = self.apis.get_mut(api_id) else {
            return 0;
        };
        sockets
            .connections
            .remove(connection_id)
            .map_or(0, |connection| connection.subscriptions.len())
    }

    /// Record that `connection_id` subscribes to `channel` under the id the client chose.
    pub fn subscribe(
        &mut self,
        api_id: &str,
        connection_id: &str,
        subscription_id: &str,
        channel: &str,
    ) -> Result<(), SubscribeRefusal> {
        let connection = self
            .apis
            .get_mut(api_id)
            .and_then(|sockets| sockets.connections.get_mut(connection_id))
            .ok_or(SubscribeRefusal::NoConnection)?;
        if connection.subscriptions.contains_key(subscription_id) {
            return Err(SubscribeRefusal::DuplicateId);
        }
        if connection.subscriptions.len() >= SUBSCRIPTIONS_PER_CONNECTION {
            return Err(SubscribeRefusal::TooMany);
        }
        connection
            .subscriptions
            .insert(subscription_id.to_string(), channel.to_string());
        Ok(())
    }

    /// Forget one subscription of `connection_id`, answering whether it held one under that
    /// id — `false` is an unsubscribe for a subscription this connection does not hold.
    pub fn unsubscribe(
        &mut self,
        api_id: &str,
        connection_id: &str,
        subscription_id: &str,
    ) -> Option<String> {
        self.apis
            .get_mut(api_id)
            .and_then(|sockets| sockets.connections.get_mut(connection_id))
            .and_then(|connection| connection.subscriptions.remove(subscription_id))
    }

    /// The channel one subscription of `connection_id` is on, or `None` when it holds none
    /// under that id.
    pub fn subscribed_channel(
        &self,
        api_id: &str,
        connection_id: &str,
        subscription_id: &str,
    ) -> Option<String> {
        self.apis
            .get(api_id)?
            .connections
            .get(connection_id)?
            .subscriptions
            .get(subscription_id)
            .cloned()
    }

    /// Every subscription of `api_id` that `channel` publishes to: the queue its frames go
    /// to, paired with the subscription id the `data` frame is addressed by.
    ///
    /// The queues are CLONED out, so a delivery is made with no store lock held.
    pub fn deliveries(&self, api_id: &str, channel: &str) -> Vec<(String, Sender<String>)> {
        let Some(sockets) = self.apis.get(api_id) else {
            return Vec::new();
        };
        sockets
            .connections
            .values()
            .flat_map(|connection| {
                connection
                    .subscriptions
                    .iter()
                    .filter(|(_, subscribed)| subscription_matches(subscribed, channel))
                    .map(|(id, _)| (id.clone(), connection.frames.clone()))
            })
            .collect()
    }

    /// How many realtime connections are open on `api_id`.
    pub fn active_connections(&self, api_id: &str) -> usize {
        self.apis
            .get(api_id)
            .map_or(0, |sockets| sockets.connections.len())
    }

    /// How many subscriptions those connections hold in total.
    pub fn active_subscriptions(&self, api_id: &str) -> usize {
        self.apis.get(api_id).map_or(0, |sockets| {
            sockets
                .connections
                .values()
                .map(|connection| connection.subscriptions.len())
                .sum()
        })
    }
}

/// What a connection id's bytes are mixed with, so one can never coincide with an event
/// identifier minted from the same counter value.
const CONNECTION_ID_MIX_DOMAIN: u64 = 0x4170_7053_796E_6301;

/// What an event identifier's bytes are mixed with ([`CONNECTION_ID_MIX_DOMAIN`] states
/// why the two differ).
const EVENT_IDENTIFIER_MIX_DOMAIN: u64 = 0x4170_7053_796E_6302;

/// What the LOW half of an id is domain-separated from the high half by, so the halves of
/// one id are not the same eight bytes twice.
const UUID_LOW_HALF_DOMAIN: u64 = 0xA5A5_A5A5_A5A5_A5A5;

/// `seed` as a UUID in the canonical 8-4-4-4-12 form, with the version and variant bits of
/// a random UUID set — the shape AppSync's own connection and event identifiers have.
///
/// The bytes come from the state's own counter through the service's existing bijective
/// mixer ([`super::splitmix64`]), so consecutive ids do not spell consecutive numbers and
/// an id is opaque to its holder; the LAYOUT is the tree's one rendering of those bytes as
/// a UUID (`service_common::uuid_from_random_bytes`), so the version and variant bits and
/// the group boundaries are not spelled out again here.
///
/// Reached by the GraphQL API's real-time plane too ([`super::graphql_data_plane`]), with a
/// domain of its own, so every connection id this service mints is minted one way.
pub(super) fn uuid_from(domain: u64, seed: u64) -> String {
    // The DOMAIN is mixed first and the seed added to the mixed value, so the two kinds of
    // id are drawn from parts of the sequence far apart from each other: mixing
    // `domain ^ seed` instead would let one kind's seed land on the other kind's domain.
    let high = super::splitmix64(super::splitmix64(domain).wrapping_add(seed)).to_be_bytes();
    let low = super::splitmix64(
        super::splitmix64(domain ^ UUID_LOW_HALF_DOMAIN).wrapping_add(seed),
    )
    .to_be_bytes();
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&high);
    bytes[8..].copy_from_slice(&low);
    ::service_common::uuid_from_random_bytes(bytes)
}

/// The segments of a channel, with the leading, trailing and repeated separators dropped:
/// a channel is written `/namespace/path`, and the segments are what name the namespace it
/// belongs to and the path within it.
pub fn channel_segments(channel: &str) -> Vec<&str> {
    channel.split('/').filter(|part| !part.is_empty()).collect()
}

/// The channel namespace a channel belongs to: its first segment, since a channel is
/// "`/namespace/channel`" and a namespace's "name must be unique within the `Api`".
///
/// `None` for a value that names no segment at all, which addresses no namespace.
pub fn channel_namespace_name(channel: &str) -> Option<&str> {
    channel_segments(channel).first().copied()
}

/// The channel as a path, normalised to the leading-separator form AppSync writes one in
/// (`/namespace/channel`), so one channel is one key however a client spelled it.
pub fn normalised_channel(channel: &str) -> String {
    format!("/{}", channel_segments(channel).join("/"))
}

/// Whether a subscription on `subscribed` receives what was published to `published`.
///
/// Segment-wise equality, with one generalisation AppSync's own channels have: a `*` as
/// the subscription's LAST segment subscribes to everything under the segments before it,
/// so `/ns/*` receives `/ns/room1` and `/ns/room1/thread`. A `*` anywhere else is a
/// segment name like any other.
pub fn subscription_matches(subscribed: &str, published: &str) -> bool {
    let wanted = channel_segments(subscribed);
    let actual = channel_segments(published);
    match wanted.split_last() {
        Some((&"*", prefix)) => actual.len() > prefix.len() && actual.starts_with(prefix),
        _ => wanted == actual,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_id_is_a_uuid_and_is_never_reused() {
        let mut plane = EventDataPlane::default();
        let mut seen: Vec<String> = Vec::new();
        // The rows span what the minting generalises over: the first id a plane mints, a
        // run of later ones, and both kinds of id, which are minted from one counter each.
        for case in 1..=8u64 {
            for minted in [plane.mint_connection_id(), plane.mint_event_identifier()] {
                let shape: Vec<usize> = minted.split('-').map(str::len).collect();
                assert_eq!(
                    shape,
                    vec![8, 4, 4, 4, 12],
                    "case {case}: expected the canonical UUID form, got {minted:?}"
                );
                assert!(
                    minted
                        .chars()
                        .all(|character| character == '-' || character.is_ascii_hexdigit()),
                    "case {case}: expected hexadecimal UUID digits, got {minted:?}"
                );
                assert_eq!(
                    minted.as_bytes()[14],
                    b'4',
                    "case {case}: expected a version-4 UUID, got {minted:?}"
                );
                assert!(
                    !seen.contains(&minted),
                    "case {case}: expected a new id, got the already-minted {minted:?}"
                );
                seen.push(minted);
            }
        }
    }

    #[test]
    fn a_channel_reads_as_its_namespace_and_its_normalised_path() {
        // The rows span the forms a client writes a channel in: with and without the
        // leading separator, a nested path, repeated separators, and a value naming no
        // segment at all.
        let cases: &[(&str, Option<&str>, &str)] = &[
            ("/default/room1", Some("default"), "/default/room1"),
            ("default/room1", Some("default"), "/default/room1"),
            ("/default/room1/thread", Some("default"), "/default/room1/thread"),
            ("//default//room1/", Some("default"), "/default/room1"),
            ("/default", Some("default"), "/default"),
            ("/", None, "/"),
        ];
        for (channel, namespace, normalised) in cases {
            assert_eq!(
                channel_namespace_name(channel),
                *namespace,
                "case {channel:?}: expected the namespace {namespace:?}, got {:?}",
                channel_namespace_name(channel)
            );
            assert_eq!(
                normalised_channel(channel),
                *normalised,
                "case {channel:?}: expected {normalised:?}, got {:?}",
                normalised_channel(channel)
            );
        }
    }

    #[test]
    fn a_subscription_receives_the_channels_it_covers() {
        // The rows span what the matching generalises over: the same channel written
        // differently, a different channel of the same namespace, a channel of another
        // namespace, a trailing wildcard over one and several segments, the namespace
        // itself under a wildcard, and a `*` that is not the last segment.
        let cases: &[(&str, &str, bool)] = &[
            ("/default/room1", "/default/room1", true),
            ("default/room1", "/default/room1", true),
            ("/default/room1", "/default/room2", false),
            ("/default/room1", "/other/room1", false),
            ("/default/*", "/default/room1", true),
            ("/default/*", "/default/room1/thread", true),
            ("/default/*", "/default", false),
            ("/default/*", "/other/room1", false),
            ("/*", "/default/room1", true),
            ("/default/*/room1", "/default/any/room1", false),
        ];
        for (subscribed, published, expected) in cases {
            let matched = subscription_matches(subscribed, published);
            assert_eq!(
                matched, *expected,
                "case ({subscribed:?}, {published:?}): expected {expected}, got {matched}"
            );
        }
    }
}
