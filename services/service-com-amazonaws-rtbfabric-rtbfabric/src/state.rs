// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rtbfabric-rtbfabric service.
//!
//! The gateway-pair + link lifecycle is the service's core loop: a requester
//! gateway and a responder gateway are created independently, and a `Link`
//! between two stored gateways is the primary resource. Everything an operation
//! answers is read back out of the records below — `activeLinksCount` /
//! `totalLinksCount` are DERIVED by scanning [`ComAmazonawsRtbfabricRtbfabricState::links`],
//! never stored.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    LinkAttributes, LinkLogSettings, ListenerConfig, ManagedEndpointConfiguration,
    ModuleConfiguration, TrustStoreConfiguration,
};

/// How long after the creating call a resource this service reports in a
/// non-terminal status becomes settled.
///
/// AWS publishes no duration for RTB Fabric gateway or link provisioning, so
/// this span is the emulator's own choice: non-zero, so a caller acting straight
/// after the create still observes the transient status, and short enough that
/// nothing has to wait long for the settled one.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// The region a request that carries no SigV4 credential scope is treated as
/// addressing, for the DNS name a gateway is assigned.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many requester gateways, and how many responder gateways, the store holds
/// before the matching create refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402).
///
/// AWS publishes adjustable per-account quotas on RTB Fabric gateways, and this
/// emulator models neither Service Quotas nor an account's adjustments, so this
/// is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It sits far above any workflow an emulator serves, and it is
/// what stops the state growing without end: `DeleteRequesterGateway` releases a
/// requester gateway's record, its links and its client tokens
/// ([`ComAmazonawsRtbfabricRtbfabricState::release_requester_gateway`]), while
/// `DeleteResponderGateway` is still a stub, so for a responder gateway this bound
/// is the only thing that holds.
pub const GATEWAY_QUOTA: usize = 1_000;

/// How many links the store holds before `CreateLink` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402).
///
/// The emulator's own bound, for the same reason as [`GATEWAY_QUOTA`].
/// `DeleteLink` releases a link, so this bounds what is live rather than what was
/// ever created.
pub const LINK_QUOTA: usize = 5_000;

/// How many `@idempotencyToken` records the store keeps before a create refuses
/// with the modelled `ServiceQuotaExceededException` (HTTP 402).
///
/// A record is written ONLY by a create that actually stored a gateway, so the
/// map cannot hold more than the two gateway quotas; the bound is checked anyway,
/// so the retention rule survives a future create that forgets it. Each record is
/// fixed size — both the key and the parameter fingerprint are 32-byte digests —
/// so a `clientToken` or a request body the model leaves unbounded in length
/// cannot grow the bytes retained per record.
pub const IDEMPOTENCY_QUOTA: usize = 2 * GATEWAY_QUOTA;

/// `RequesterGatewayStatus`/`ResponderGatewayStatus` member: creation accepted,
/// the gateway is not usable yet.
pub const GATEWAY_STATUS_PENDING_CREATION: &str = "PENDING_CREATION";
/// `RequesterGatewayStatus`/`ResponderGatewayStatus` member: the gateway is usable.
pub const GATEWAY_STATUS_ACTIVE: &str = "ACTIVE";
/// `RequesterGatewayStatus`/`ResponderGatewayStatus` member: terminal, the
/// gateway is gone.
pub const GATEWAY_STATUS_DELETED: &str = "DELETED";

/// The message this service's `ResourceNotFoundException` carries: fixed text
/// naming neither the resource asked for nor its id.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource was not found.";

/// `LinkStatus` member: the link request is accepted and not yet put to the peer.
pub const LINK_STATUS_PENDING_REQUEST: &str = "PENDING_REQUEST";
/// `LinkStatus` member: the link request is outstanding against the peer.
pub const LINK_STATUS_REQUESTED: &str = "REQUESTED";
/// `LinkStatus` member: the link carries traffic.
pub const LINK_STATUS_ACTIVE: &str = "ACTIVE";
/// `LinkStatus` member: terminal, the peer declined the link request.
pub const LINK_STATUS_REJECTED: &str = "REJECTED";
/// `LinkStatus` member: terminal, the link is gone.
pub const LINK_STATUS_DELETED: &str = "DELETED";

/// `LinkDirection` member: the link was opened by a requester gateway.
pub const LINK_DIRECTION_REQUEST: &str = "REQUEST";
/// `LinkDirection` member: the link was opened by a responder gateway.
pub const LINK_DIRECTION_RESPONSE: &str = "RESPONSE";

/// `ConnectivityType` member: both ends are gateways in this account's VPCs.
pub const CONNECTIVITY_TYPE_DEFAULT: &str = "DEFAULT";

/// `GatewayType` member: the gateway accepts links from outside the fabric.
pub const GATEWAY_TYPE_EXTERNAL: &str = "EXTERNAL";

/// A requester gateway, as `CreateRequesterGateway` persisted it.
#[derive(Debug, Clone)]
pub struct RequesterGateway {
    pub gateway_id: String,
    pub domain_name: String,
    pub status: String,
    /// The instant the clock makes `PENDING_CREATION` -> `ACTIVE` due; `None`
    /// once the record has settled.
    pub settles_at: Option<SystemTime>,
    pub vpc_id: String,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl RequesterGateway {
    /// Materialise the `PENDING_CREATION` -> `ACTIVE` transition the clock has
    /// already made due, dated at the instant it was owed rather than now, so
    /// the record does not depend on when it was next read.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.settles_at.filter(|due| *due <= now) {
            self.status = GATEWAY_STATUS_ACTIVE.to_string();
            self.settles_at = None;
            self.updated_at = due;
        }
    }
}

/// A responder gateway, as `CreateResponderGateway` persisted it.
#[derive(Debug, Clone)]
pub struct ResponderGateway {
    pub gateway_id: String,
    pub status: String,
    /// See [`RequesterGateway::settles_at`].
    pub settles_at: Option<SystemTime>,
    pub vpc_id: String,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    /// The domain name the caller named, if it named one. A gateway reached
    /// through a managed endpoint instead has none, so this is never synthesised.
    pub domain_name: Option<String>,
    pub port: i32,
    pub protocol: String,
    pub listener_config: Option<ListenerConfig>,
    pub trust_store_configuration: Option<TrustStoreConfiguration>,
    pub managed_endpoint_configuration: Option<ManagedEndpointConfiguration>,
    pub gateway_type: Option<String>,
    pub client_routing_policy: Option<String>,
    pub external_inbound_endpoint: Option<String>,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ResponderGateway {
    /// See [`RequesterGateway::settle`].
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.settles_at.filter(|due| *due <= now) {
            self.status = GATEWAY_STATUS_ACTIVE.to_string();
            self.settles_at = None;
            self.updated_at = due;
        }
    }
}

/// A link between two stored gateways, as `CreateLink` persisted it.
///
/// Holds the members `CreateLink` takes but its response shape does not carry
/// (`tags`, `httpResponderAllowed`, `timeoutInMillis`): `GetLink` reads them
/// back from here, which is the whole point of storing the request rather than
/// echoing it.
#[derive(Debug, Clone)]
pub struct Link {
    pub link_id: String,
    pub gateway_id: String,
    pub peer_gateway_id: String,
    pub status: String,
    /// The instant the clock makes `PENDING_REQUEST` -> `REQUESTED` due; `None`
    /// once the record has settled.
    pub settles_at: Option<SystemTime>,
    pub direction: String,
    pub connectivity_type: String,
    pub attributes: Option<LinkAttributes>,
    pub log_settings: LinkLogSettings,
    pub http_responder_allowed: Option<bool>,
    pub timeout_in_millis: Option<i64>,
    pub flow_modules: Vec<ModuleConfiguration>,
    pub pending_flow_modules: Vec<ModuleConfiguration>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl Link {
    /// Materialise the `PENDING_REQUEST` -> `REQUESTED` transition the clock has
    /// already made due: the request has reached the peer and is waiting on it.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.settles_at.filter(|due| *due <= now) {
            self.status = LINK_STATUS_REQUESTED.to_string();
            self.settles_at = None;
            self.updated_at = due;
        }
    }

    /// Whether `gateway_id` is one of this link's two ends — either end may read
    /// or delete it, so `GET /gateway/{gatewayId}/link/{linkId}` resolves from
    /// the requester's and the responder's id alike.
    pub fn has_end(&self, gateway_id: &str) -> bool {
        self.gateway_id == gateway_id || self.peer_gateway_id == gateway_id
    }

    /// This link's OTHER end, seen from `gateway_id`: a link is one record with
    /// two ends, so a response addressed to one of them reports the other as its
    /// `peerGatewayId`. An id that is neither end never reaches here, because the
    /// link is resolved by [`has_end`](Self::has_end) first.
    pub fn other_end(&self, gateway_id: &str) -> &str {
        if self.gateway_id == gateway_id {
            &self.peer_gateway_id
        } else {
            &self.gateway_id
        }
    }

    /// Whether the link request is still outstanding — accepted by this service
    /// but not yet answered by the peer. These are the only statuses from which
    /// the peer can still accept or decline the request.
    pub fn request_outstanding(&self) -> bool {
        self.status == LINK_STATUS_PENDING_REQUEST || self.status == LINK_STATUS_REQUESTED
    }
}

/// What the first call carrying an `@idempotencyToken` minted, so a retry with
/// the same token answers with the same resource instead of a second one.
///
/// Fixed size: see [`IDEMPOTENCY_QUOTA`] for the retention rule.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    /// The id the first call minted.
    pub resource_id: String,
    /// A digest of that call's other parameters: the documented contract is that
    /// a retry of the same token with DIFFERENT parameters fails rather than
    /// silently answering the first call's resource.
    pub fingerprint: [u8; 32],
}

/// The number of links a gateway is an end of, by status. Every member is
/// derived by scanning [`ComAmazonawsRtbfabricRtbfabricState::links`].
#[derive(Debug, Default, Clone, Copy)]
pub struct LinkCounts {
    /// Every link this gateway is an end of.
    pub total: i32,
    /// Those carrying traffic.
    pub active: i32,
    /// Those whose request is still outstanding.
    pub requested: i32,
}

/// Per-service state for com-amazonaws-rtbfabric-rtbfabric. A handler reaches it through [`service_state`].
///
/// Every map below is bounded: gateways by [`GATEWAY_QUOTA`], links by
/// [`LINK_QUOTA`] and the idempotency records by [`IDEMPOTENCY_QUOTA`], each
/// checked by the create that would insert, BEFORE it inserts. That is also what
/// bounds the per-request work the linear scans here do under the state lock.
#[derive(Debug, Default)]
pub struct ComAmazonawsRtbfabricRtbfabricState {
    /// Keyed by `gatewayId`. Bounded by [`GATEWAY_QUOTA`]; released by
    /// `DeleteRequesterGateway` through
    /// [`release_requester_gateway`](Self::release_requester_gateway).
    pub requester_gateways: HashMap<String, RequesterGateway>,
    /// Keyed by `gatewayId`. Bounded by [`GATEWAY_QUOTA`]; released by
    /// `DeleteResponderGateway` once that is implemented.
    pub responder_gateways: HashMap<String, ResponderGateway>,
    /// Keyed by (the `gatewayId` the link was opened from, `linkId`). Bounded by
    /// [`LINK_QUOTA`]; released by `DeleteLink`.
    pub links: HashMap<(String, String), Link>,
    /// Keyed by [`idempotency_key`], which folds the operation and the caller's
    /// `clientToken` into one 32-byte digest: a token is scoped to the operation
    /// it was presented to. Bounded by [`IDEMPOTENCY_QUOTA`], and written only by
    /// a create that stored a resource.
    pub idempotency: HashMap<[u8; 32], IdempotentCreate>,
    /// Monotonic source of the id suffixes this emulator instance mints, so no
    /// two gateways or links ever share an id.
    pub next_id: u64,
}

impl ComAmazonawsRtbfabricRtbfabricState {
    /// The next id suffix, unique within this emulator instance.
    fn next_suffix(&mut self) -> String {
        self.next_id = self.next_id.saturating_add(1);
        format!("{:016x}", self.next_id)
    }

    /// A fresh `gatewayId`, matching the modelled `^rtb-gw-[a-z0-9-]{1,25}$`.
    pub fn mint_gateway_id(&mut self) -> String {
        format!("rtb-gw-{}", self.next_suffix())
    }

    /// A fresh `linkId`, matching the modelled `^link-[a-z0-9-]{1,25}$`.
    pub fn mint_link_id(&mut self) -> String {
        format!("link-{}", self.next_suffix())
    }

    /// Whether `gateway_id` names a stored gateway of either kind.
    pub fn gateway_exists(&self, gateway_id: &str) -> bool {
        self.requester_gateways.contains_key(gateway_id)
            || self.responder_gateways.contains_key(gateway_id)
    }

    /// Apply every status transition the clock has already made due.
    ///
    /// A reporting operation calls this before it answers: the change was owed
    /// at this same instant whether or not anybody asked, so materialising it
    /// here is not the read mutating the resource.
    pub fn settle_all(&mut self, now: SystemTime) {
        for gateway in self.requester_gateways.values_mut() {
            gateway.settle(now);
        }
        for gateway in self.responder_gateways.values_mut() {
            gateway.settle(now);
        }
        for link in self.links.values_mut() {
            link.settle(now);
        }
    }

    /// The link `link_id` between `gateway_id` and its peer, addressed from
    /// either end.
    pub fn find_link(&self, gateway_id: &str, link_id: &str) -> Option<&Link> {
        self.links
            .values()
            .find(|link| link.link_id == link_id && link.has_end(gateway_id))
    }

    /// [`find_link`](Self::find_link)'s link, for an operation that moves it to
    /// another status.
    pub fn find_link_mut(&mut self, gateway_id: &str, link_id: &str) -> Option<&mut Link> {
        self.links
            .values_mut()
            .find(|link| link.link_id == link_id && link.has_end(gateway_id))
    }

    /// The map key of [`find_link`](Self::find_link)'s link, for removing it.
    pub fn find_link_key(&self, gateway_id: &str, link_id: &str) -> Option<(String, String)> {
        self.links
            .iter()
            .find(|(_, link)| link.link_id == link_id && link.has_end(gateway_id))
            .map(|(key, _)| key.clone())
    }

    /// Whether a link already joins these two gateways, in either orientation: a
    /// link is one record with two ends, readable and deletable from both, so the
    /// reverse create would be a second record for the same pair.
    pub fn link_joining(&self, gateway_id: &str, peer_gateway_id: &str) -> Option<&Link> {
        self.links
            .values()
            .find(|link| link.has_end(gateway_id) && link.has_end(peer_gateway_id))
    }

    /// Count the stored links `gateway_id` is an end of, by status.
    pub fn link_counts(&self, gateway_id: &str) -> LinkCounts {
        let mine = self.links.values().filter(|link| link.has_end(gateway_id));
        let mut counts = LinkCounts::default();
        for link in mine {
            counts.total = counts.total.saturating_add(1);
            if link.status == LINK_STATUS_ACTIVE {
                counts.active = counts.active.saturating_add(1);
            }
            if link.request_outstanding() {
                counts.requested = counts.requested.saturating_add(1);
            }
        }
        counts
    }

    /// Release the requester gateway `gateway_id`: everything the store holds
    /// BECAUSE of that gateway goes with it, so no later read can report a
    /// resource whose gateway is gone.
    ///
    /// `None` when no requester gateway of that id is stored, which is the
    /// not-found the caller is answered with. Otherwise the record removed, and
    /// with it:
    ///
    /// * every link the gateway was an end of — a link is one record joining two
    ///   stored gateways and cannot outlive either of them;
    /// * every client-token record that minted it, so the id a replayed token
    ///   would answer with is never one the store no longer holds.
    pub fn release_requester_gateway(&mut self, gateway_id: &str) -> Option<RequesterGateway> {
        let released = self.requester_gateways.remove(gateway_id)?;
        self.links.retain(|_, link| !link.has_end(gateway_id));
        self.idempotency
            .retain(|_, record| record.resource_id != gateway_id);
        Some(released)
    }
}

/// The modelled timestamp for an instant held in state.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The digest an idempotency record is keyed by: the operation the token was
/// presented to, and the token itself. Hashing both keeps the retained key a
/// fixed 32 bytes however long a `clientToken` the caller chose — the model
/// bounds neither its length nor its content.
pub fn idempotency_key(operation: &str, client_token: &str) -> [u8; 32] {
    digest(&[operation.as_bytes(), client_token.as_bytes()])
}

/// The digest of a create's parameters, which an `@idempotencyToken` retry is
/// compared against. Fixed size, so a retained record never grows with the
/// request it came from.
pub fn parameter_fingerprint(rendering: &str) -> [u8; 32] {
    digest(&[rendering.as_bytes()])
}

/// SHA-256 over `parts`, each length-prefixed so two different splits of the
/// same bytes cannot collide.
fn digest(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// A tag map rendered canonically (sorted by key), so the same tags in a
/// different iteration order fingerprint identically.
///
/// A fingerprint is only ever compared with one computed by the same build in
/// the same process, so the rest of a request can be rendered with `Debug`; a
/// `HashMap` cannot, because its iteration order varies between two calls that
/// carry identical tags.
pub fn canonical_tags(tags: Option<&HashMap<String, String>>) -> String {
    let mut pairs: Vec<(&str, &str)> = tags
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    pairs.sort_unstable();
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// One DNS label, from arbitrary text: lowercase, only `[a-z0-9-]`, and never
/// leading or trailing `-`, which is what the modelled `DomainName` pattern
/// accepts for each dot-separated label.
fn dns_label(text: &str) -> Option<String> {
    let kept: String = text
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let trimmed = kept.trim_matches('-');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The region the caller addressed, from the request's SigV4 credential scope;
/// [`DEFAULT_REGION`] when the request carries no scope (an anonymous call).
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .and_then(|scope| dns_label(&scope.region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The DNS name this service assigns a gateway: the gateway's own id as the
/// leftmost label under the service's regional zone.
pub fn gateway_domain_name(gateway_id: &str, region: &str) -> String {
    format!("{gateway_id}.rtbfabric.{region}.amazonaws.com")
}

/// The public name an `EXTERNAL` responder gateway accepts inbound external
/// links on. An `INTERNAL` gateway has none.
pub fn external_inbound_endpoint(gateway_id: &str, region: &str) -> String {
    format!("{gateway_id}.inbound.rtbfabric.{region}.amazonaws.com")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRtbfabricRtbfabricState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRtbfabricRtbfabricState>)
}
