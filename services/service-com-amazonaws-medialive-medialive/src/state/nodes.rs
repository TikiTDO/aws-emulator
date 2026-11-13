// SPDX-License-Identifier: CC0-1.0

//! The node slice of this service's state — the MediaLive Anywhere Node.
//!
//! One file for the whole slice — the record, its bounds, its refusals, the
//! store's methods and the output projection — so the slice can be read, and
//! merged, on its own, the same way the multiplex slice is laid out. The store
//! itself is the `nodes` member of [`ComAmazonawsMedialiveMedialiveState`] in
//! the parent module, beside the channels and inputs, so every operation that
//! reaches a node reads the one copy.
//!
//! A node is addressed by the CLUSTER it belongs to and its own id (every node
//! operation is bound under `/prod/clusters/{ClusterId}/nodes/...` and `Id` is
//! documented as "Unique in the Cluster"), so every lookup here takes both and a
//! node id presented under another cluster resolves to nothing. The cluster has
//! to be one this store HOLDS: `CreateNode` "Create a Node in the specified
//! Cluster", so [`ComAmazonawsMedialiveMedialiveState::insert_node`] resolves
//! the id against the cluster slice beside this one and refuses a cluster the
//! account does not have.
//!
//! Every value a node-shaped response carries is either stored verbatim from the
//! request that created the node or derived from the stored state
//! ([`ComAmazonawsMedialiveMedialiveState::node_summary`]) — no operation builds
//! a node out of its own request.
//!
//! A node's `State` is the one member a CALLER moves: `UpdateNodeState` is what
//! commissions a node or isolates it
//! ([`ComAmazonawsMedialiveMedialiveState::apply_node_state`]), and every other
//! value of the `NodeState` enum is either where a create starts
//! ([`node_state::CREATED`]), what a deletion walks through, what the service's
//! OWN work puts the node in while a channel runs on it
//! ([`ComAmazonawsMedialiveMedialiveState::reported_node_state`]), or a state
//! only the node's own hardware could reach (see [`node_state`]).
//!
//! A node's SETTINGS — its `Name`, its `Role` and its SDI source mappings — are
//! what `UpdateNode` changes
//! ([`ComAmazonawsMedialiveMedialiveState::apply_node_changes`]): a member the
//! request leaves out is not changed, a `Name` is unique in the cluster the node
//! belongs to, and an SDI source mapping has to name all three of its documented
//! parameters, a card the hardware numbering reaches and an SdiSource this
//! account holds.
//!
//! A node's `Name` is unique among the live nodes of its cluster whichever write
//! set it, so BOTH writes resolve a requested name against
//! [`ComAmazonawsMedialiveMedialiveState::node_named`] and refuse one another
//! live node of that cluster already holds: the create stores nothing and the
//! update changes nothing.
//!
//! A node's DELETION is a DEREGISTRATION: the `NodeState` enum models no
//! `DELETING`/`DELETED` pair, it models `DEREGISTERING` and the terminal
//! `DEREGISTERED`, which is what `DeleteNode`'s `202 Accepted` accepts and what
//! the clock then settles the node into ([`NodeRecord::settle`]). It is accepted
//! only for a node that is IDLE — a node still running a channel is refused
//! before anything is written
//! ([`ComAmazonawsMedialiveMedialiveState::deregister_node_description`]), the
//! same precondition the cluster's own delete enforces. As everywhere
//! else in this service a delete never drops the record: the node stays readable
//! in its terminal state instead of vanishing. The instant is STORED and never
//! slept on — [`ComAmazonawsMedialiveMedialiveState::settle`], which every
//! operation runs first, is what materialises the transitions the clock has
//! already made due.

use std::collections::HashMap;
use std::time::SystemTime;

use crate::ops::types::{
    DescribeNodeSummary, NodeInterfaceMapping, NodeInterfaceMappingCreateRequest, SdiSourceMapping,
    SdiSourceMappingUpdateRequest,
};

use super::{ComAmazonawsMedialiveMedialiveState, SdiSourceRecord, TRANSIENT_STATE_SPAN};

/// The `NodeState` values this slice uses, spelled exactly as the model declares
/// them (`smithy_query(query="shape", shape="NodeState")`: CREATED | REGISTERING
/// | READY_TO_ACTIVATE | REGISTRATION_FAILED | ACTIVATION_FAILED | ACTIVE |
/// READY | IN_USE | DEREGISTERING | DRAINING | DEREGISTRATION_FAILED |
/// DEREGISTERED).
///
/// The six the model declares but this slice does not name are the ones
/// nothing here can reach. A node's registration and activation happen on node
/// HARDWARE running the Elemental Anywhere software, which this emulator does
/// not run: nothing registers (`REGISTERING`, `READY_TO_ACTIVATE`,
/// `REGISTRATION_FAILED`) and nothing reports a readiness it learnt from that
/// software (`ACTIVATION_FAILED`, `READY`). `DEREGISTRATION_FAILED` reports a
/// failure of that same software, which this emulator never fabricates either.
///
/// `IN_USE` IS reached, and is derived rather than stored
/// ([`ComAmazonawsMedialiveMedialiveState::reported_node_state`]): a node is in
/// use exactly while a channel placed in one of the ChannelPlacementGroups it
/// is assigned to is running, which is the state this emulator does model.
///
/// `ACTIVE` and `DRAINING` are the two the CALLER asks for: they are exactly the
/// value set of `UpdateNodeStateShape` (`smithy_query(query="shape",
/// shape="UpdateNodeStateShape")`), the member that operation uses to commission
/// a node or isolate it, so they are reached by that request rather than learnt
/// from hardware.
pub mod node_state {
    /// Where every created node starts: `CreateNode` "Create a Node in the
    /// specified Cluster", and the node then has to run its registration script
    /// on the hardware that will host it before it is anything else.
    pub const CREATED: &str = "CREATED";
    /// A node the caller has COMMISSIONED: `UpdateNodeState`'s `State` "Set to
    /// ACTIVE (COMMISSIONED) to indicate that the Node is deployable".
    pub const ACTIVE: &str = "ACTIVE";
    /// A node the caller has ISOLATED: `UpdateNodeState`'s `State` "Set to
    /// DRAINING to isolate the Node so that MediaLive Anywhere won't use it".
    pub const DRAINING: &str = "DRAINING";
    /// A node a channel is RUNNING on, which is what the node's delete is
    /// refused on ("Delete a Node. The Node must be IDLE").
    pub const IN_USE: &str = "IN_USE";
    /// What `DeleteNode` accepts a node into, and what the clock settles.
    pub const DEREGISTERING: &str = "DEREGISTERING";
    /// The terminal state a deregistered node keeps.
    pub const DEREGISTERED: &str = "DEREGISTERED";
}

/// The `NodeConnectionState` a node in this emulator reports
/// (`smithy_query(query="shape", shape="NodeConnectionState")`: CONNECTED |
/// DISCONNECTED).
///
/// A node is `CONNECTED` once the Elemental Anywhere software on its hardware has
/// connected to the service. This emulator runs no node hardware and accepts no
/// connection from one, so a node here is never connected, and reporting
/// `CONNECTED` would claim a running agent that does not exist.
pub const NODE_CONNECTION_STATE: &str = "DISCONNECTED";

/// The id the first node of a process gets.
///
/// MediaLive resource ids are numeric strings, so they are minted from a counter
/// at a fixed seven-digit width, which keeps the `BTreeMap` key order equal to
/// the mint order and makes a later `ListNodes` pagination deterministic. A range
/// of its own keeps the service's id spaces apart — the cluster slice's own
/// range starts one million below this one — so a caller (or a test) that passes
/// a cluster's or a channel's id where a node id belongs gets a
/// `NotFoundException` rather than a coincidental hit.
const FIRST_NODE_ID: u64 = 9_000_000;

/// The most nodes this emulator stores across every cluster.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateNode` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_NODES: usize = 100;

/// The most `RequestId` entries kept for `CreateNode` idempotency — the same
/// bound, for the same reason, as [`super::MAX_IDEMPOTENCY_TOKENS`] on channels.
pub const MAX_NODE_TOKENS: usize = MAX_NODES;

/// The most `NodeInterfaceMappings` entries this emulator accepts on one node.
///
/// A ceiling, not a guess at AWS's published quota: the list is caller-chosen and
/// the model targets it at `__listOfNodeInterfaceMappingCreateRequest`, which
/// carries no `@length`, so the request boundary refuses no length and this is
/// where the bound lives — for the same reason
/// [`super::MAX_INPUT_ATTACHMENTS`] bounds a channel's attachments. `CreateNode`
/// models `BadRequestException`, which is the refusal for a request that is out
/// of bounds.
pub const MAX_NODE_INTERFACE_MAPPINGS: usize = 32;

/// The most `SdiSourceMappings` entries this emulator accepts on one node.
///
/// A ceiling, not a guess at AWS's published quota: the list is caller-chosen
/// and the model targets it at `SdiSourceMappingsUpdateRequest`, which carries
/// no `@length`, so the request boundary refuses no length and this is where the
/// bound lives — the same reason, and the same ceiling, as
/// [`MAX_NODE_INTERFACE_MAPPINGS`] on the other caller-chosen list a node
/// carries.
pub const MAX_NODE_SDI_SOURCE_MAPPINGS: usize = 32;

/// The lowest `CardNumber` a mapping can name: "A number that uniquely
/// identifies the SDI card on the node hardware. … The numbering always starts
/// at 1" (`smithy_query(query="doc", shape="SdiSourceMappingUpdateRequest",
/// member="CardNumber")`), so a number below it identifies no card.
///
/// A port's `ChannelNumber` has no documented first number — "For information
/// about how ports are identified on physical cards, see the documentation for
/// your node hardware" — so no bound of its own is invented here.
pub const FIRST_SDI_CARD_NUMBER: i32 = 1;

/// The message an `SdiSourceMappings` entry that leaves one of its three
/// members out is refused with: `SdiSourceMappingUpdateRequest` is documented
/// "You must specify all three parameters in this object", so an entry naming
/// fewer connects no port to any source.
pub const SDI_SOURCE_MAPPING_MEMBERS_MESSAGE: &str =
    "every entry of sdiSourceMappings must specify cardNumber, channelNumber and sdiSource";

/// The message a request that lists more `NodeInterfaceMappings` than this
/// emulator stores is refused with: how many the request carried and the
/// ceiling.
///
/// ONE copy for the two operations whose request carries that list —
/// `CreateNode` and `CreateNodeRegistrationScript`, which name the member
/// identically and store it under the same ceiling — so the bound they enforce
/// cannot be worded two ways, the same reason
/// [`too_many_sdi_source_mappings_message`] is one function.
pub fn too_many_interface_mappings_message(count: usize) -> String {
    format!(
        "nodeInterfaceMappings carries {count} mappings; at most {MAX_NODE_INTERFACE_MAPPINGS} are accepted"
    )
}

/// The message a request that lists more `SdiSourceMappings` than this emulator
/// stores is refused with: how many the request carried and the ceiling, worded
/// as `CreateNode` words the same class of bound on a node's other
/// caller-chosen list.
pub fn too_many_sdi_source_mappings_message(count: usize) -> String {
    format!(
        "sdiSourceMappings carries {count} mappings; at most {MAX_NODE_SDI_SOURCE_MAPPINGS} are accepted"
    )
}

/// The message a mapping naming a card below [`FIRST_SDI_CARD_NUMBER`] is
/// refused with: the number the request named and the first one the hardware
/// numbering uses.
pub fn card_number_out_of_range_message(card_number: i32) -> String {
    format!(
        "sdiSourceMappings names cardNumber {card_number}; SDI card numbering starts at {FIRST_SDI_CARD_NUMBER}"
    )
}

/// Why [`ComAmazonawsMedialiveMedialiveState::apply_node_state`] wrote nothing.
/// Each variant leaves the stored node exactly as it was.
#[derive(Debug)]
pub enum NodeStateRefusal {
    /// The cluster and node id pair names no node at all — an id no node ever
    /// had, or one presented under a cluster it does not belong to.
    NoSuchNode,
    /// The node exists but its deregistration has already been accepted, so it
    /// is readable and no longer writable.
    DeleteAccepted,
}

/// Why [`ComAmazonawsMedialiveMedialiveState::deregister_node_description`]
/// accepted no delete. Each variant leaves the stored node exactly as it was.
#[derive(Debug)]
pub enum NodeDeleteRefusal {
    /// The cluster and node id pair names no node at all — an id no node ever
    /// had, or one presented under a cluster it does not belong to.
    NoSuchNode,
    /// Channels are still running on the node, so it is not idle. Carries their
    /// ids as the refusal names them, in the order
    /// [`ComAmazonawsMedialiveMedialiveState::channels_running_on_node`]
    /// derives them.
    NotIdle { channels: Vec<String> },
}

/// The message a delete refused on a node that is still running channels
/// carries: the node the REQUEST named, how many channels are running on it and
/// which they are, then the rule the delete is held to ("Delete a Node. The
/// Node must be IDLE").
///
/// The same shape as the cluster side's refusal for the same rejection class
/// ([`super::cluster_not_idle_message`]), so the two deletes of this family
/// cannot word "this resource is not idle" two ways. Every value is read off
/// the request and the store.
pub fn node_not_idle_message(node_id: &str, channels: &[String]) -> String {
    format!(
        "Node {node_id} still has {} channel(s) running on it ({}); a node must be idle \
         before it can be deleted",
        channels.len(),
        channels.join(", ")
    )
}

/// Why [`ComAmazonawsMedialiveMedialiveState::apply_node_changes`] wrote
/// nothing. Each variant leaves the stored node exactly as it was.
#[derive(Debug)]
pub enum NodeUpdateRefusal {
    /// The cluster and node id pair names no node at all — an id no node ever
    /// had, or one presented under a cluster it does not belong to.
    NoSuchNode,
    /// The node exists but its deregistration has already been accepted, so it
    /// is readable and no longer writable.
    DeleteAccepted,
    /// Another live node of the same cluster already carries the requested
    /// `Name`, which is documented to be unique in the Cluster.
    NameTaken,
    /// The request listed more `SdiSourceMappings` than
    /// [`MAX_NODE_SDI_SOURCE_MAPPINGS`]. Carries the count it asked for, so the
    /// refusal can name it.
    TooManySdiSourceMappings(usize),
    /// One `SdiSourceMappings` entry left out at least one of the three members
    /// the shape documents as mandatory.
    IncompleteSdiSourceMapping,
    /// One entry named a `CardNumber` below [`FIRST_SDI_CARD_NUMBER`], which
    /// identifies no SDI card. Carries the number it named.
    CardNumberOutOfRange(i32),
    /// One entry's `SdiSource` names no SdiSource this account holds, so the
    /// mapping would connect a port to a source no read of the account can
    /// show. Carries the id it named.
    UnknownSdiSource(String),
}

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_node`] stored nothing. Each
/// variant is one refusal `CreateNode` models, and each leaves the store exactly
/// as it was.
#[derive(Debug)]
pub enum NodeRefusal {
    /// The account holds no cluster with the id the create was addressed to, so
    /// there is no cluster to create the node in.
    ClusterNotFound,
    /// The request listed more interface mappings than
    /// [`MAX_NODE_INTERFACE_MAPPINGS`]. Carries the count it asked for, so the
    /// refusal can name it.
    TooManyInterfaceMappings(usize),
    /// The store already holds [`MAX_NODES`] nodes.
    StoreFull,
    /// Another live node of the same cluster already carries the requested
    /// `Name`, which is documented to be unique in the Cluster.
    NameTaken,
}

/// One created node, as `CreateNode` stored it plus its lifecycle state.
///
/// The members mirror the model's `DescribeNodeSummary` shape minus the ones it
/// derives or does not claim: `State` and `ConnectionState` are the lifecycle
/// below, and `InstanceArn` ("The ARN of the EC2 instance hosting the Node"),
/// `ManagedInstanceId` and a mapping's `PhysicalInterfaceIpAddresses` ("The IP
/// addresses associated with the physical interface on the node hardware") all
/// report hardware the real service registers and this emulator does not run, so
/// none of them is invented here.
#[derive(Debug, Clone)]
pub struct NodeRecord {
    pub id: String,
    pub arn: String,
    /// "The ID of the Cluster that the Node belongs to" — the cluster the create
    /// was addressed to. A node cannot move cluster ("Note that you can't move a
    /// Node to another Cluster"), so this is written once.
    pub cluster_id: String,
    pub name: Option<String>,
    /// The `NodeRole` the request named, stored verbatim: the model documents no
    /// default for it, so a create that names none leaves it unreported rather
    /// than having one chosen here.
    pub role: Option<String>,
    /// The interface mappings the create listed, in the order it listed them,
    /// projected onto the response shape once at create time.
    pub node_interface_mappings: Vec<NodeInterfaceMapping>,
    /// The node's SDI source mappings. Empty at create time — `CreateNode`
    /// models no member for them — and written by `UpdateNode`, the one
    /// operation whose request carries them ("you create a mapping for an SDI
    /// source by updating a Node. You can't create the mapping when you first
    /// create the Node").
    pub sdi_source_mappings: Vec<SdiSourceMapping>,
    /// The request's `Tags`. `CreateNode` accepts them but no node-shaped
    /// response carries a `Tags` member (`smithy_query(query="shape",
    /// shape="DescribeNodeSummary")`), so the tag operations are what report
    /// them: `ListTagsForResource` reads this map through
    /// [`ComAmazonawsMedialiveMedialiveState::resource_tags`].
    pub tags: HashMap<String, String>,
    /// The node's current `NodeState`.
    state: String,
    /// The transition the clock owes this node, while it is in a transient
    /// state; `None` once it has settled. One member rather than one instant per
    /// state, because a node is in at most one transient state at a time.
    transition: Option<NodeTransition>,
}

impl NodeRecord {
    /// Whether a delete has already been accepted for this node: it is
    /// deregistering, or has reached the terminal `DEREGISTERED`.
    ///
    /// This is what keeps a node whose delete was accepted READABLE but not
    /// writable: the tag writers resolve a node's ARN through
    /// `ComAmazonawsMedialiveMedialiveState::resource_tags_mut`, which does not
    /// hand out a node this answers `true` for, while the READ path goes on
    /// reporting the record in its terminal state.
    /// [`ComAmazonawsMedialiveMedialiveState::apply_node_state`] resolves its
    /// node the same way, and so does
    /// [`ComAmazonawsMedialiveMedialiveState::apply_node_changes`].
    pub fn is_delete_accepted(&self) -> bool {
        self.state == node_state::DEREGISTERING || self.state == node_state::DEREGISTERED
    }

    /// Whether this node's deregistration has come due, which is what puts it in
    /// the terminal `DEREGISTERED` state it then keeps — and what takes it out
    /// of the cluster it was registered with
    /// ([`ComAmazonawsMedialiveMedialiveState::resources_in_cluster`]), since a
    /// node in that state is no longer a resource the cluster holds.
    pub fn is_deregistered(&self) -> bool {
        self.state == node_state::DEREGISTERED
    }

    /// Accept a delete for this node: move it into `DEREGISTERING` and owe the
    /// terminal `DEREGISTERED` once [`TRANSIENT_STATE_SPAN`] has passed.
    ///
    /// A delete already accepted is left exactly as it is, so a repeated call can
    /// neither restart the clock nor revive a node the clock has already
    /// deregistered.
    fn begin_deregister(&mut self, now: SystemTime) {
        if self.is_delete_accepted() {
            return;
        }
        self.state = node_state::DEREGISTERING.to_string();
        self.transition = Some(NodeTransition {
            due: now + TRANSIENT_STATE_SPAN,
            to: node_state::DEREGISTERED,
        });
    }

    /// Put this node into the state its caller asked `UpdateNodeState` for.
    ///
    /// The state is the one the request named — the operation's `State` member
    /// is what commissions a node (`ACTIVE`) or isolates it (`DRAINING`), and
    /// both are states a node STAYS in until something asks for another one, so
    /// the node is in it as of this call and the clock owes nothing after it.
    /// Any transition still owed is dropped for that reason: the state the
    /// caller asked for is the node's current one, and an older owed step must
    /// not overwrite it later. Today no node this is reachable for has one — the
    /// deregistration is the only transition a node records, and
    /// [`ComAmazonawsMedialiveMedialiveState::apply_node_state`] refuses a node
    /// whose deregistration was accepted — so the clear is what keeps the
    /// invariant true of a transition kind the slice gains later rather than a
    /// step it drops now.
    fn set_requested_state(&mut self, state: &str) {
        self.state = state.to_string();
        self.transition = None;
    }

    /// Materialise the transition the clock has already made due: a node
    /// `DeleteNode` accepted into `DEREGISTERING` becomes the terminal
    /// `DEREGISTERED` once the span has passed.
    ///
    /// Answers whether THIS call is what settled the node into that terminal
    /// state, which is what ends the idempotency window of the tokens that
    /// created it
    /// ([`ComAmazonawsMedialiveMedialiveState::settle_nodes`]).
    fn settle(&mut self, now: SystemTime) -> bool {
        let Some(transition) = self.transition else {
            return false;
        };
        if now < transition.due {
            return false;
        }
        self.state = transition.to.to_string();
        self.transition = None;
        self.is_deregistered()
    }
}

/// A transition the clock owes a node: the instant it becomes due and the
/// `NodeState` it settles into. Stored, never slept on — the same shape as the
/// channel and multiplex slices' own transitions, for the same reason.
#[derive(Debug, Clone, Copy)]
struct NodeTransition {
    due: SystemTime,
    to: &'static str,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every node transition the clock had already made due as of
    /// `now`. Called from [`ComAmazonawsMedialiveMedialiveState::settle`], which
    /// every operation runs first.
    pub(super) fn settle_nodes(&mut self, now: SystemTime) {
        // Only a node that settles into the terminal `DEREGISTERED` on THIS call
        // is collected, so the work here does not grow with the number of nodes
        // that have already reached that state.
        let mut just_deregistered: Vec<String> = Vec::new();
        for (id, node) in &mut self.nodes {
            if node.settle(now) {
                just_deregistered.push(id.clone());
            }
        }
        // A token never outlives the node it minted: once the node's
        // deregistration has come due it is no longer a resource the cluster
        // holds, so its idempotency window is over and a retried create mints a
        // new node rather than answering with a deregistered one — the rule the
        // cluster and channel placement group tokens of the same family follow.
        for id in just_deregistered {
            self.node_tokens.forget(&id);
        }
    }

    /// The next node id, as AWS shapes MediaLive resource ids: a numeric string.
    fn mint_node_id(&mut self) -> String {
        (FIRST_NODE_ID + self.mints.next(|mints| &mut mints.node)).to_string()
    }

    /// Store a node created in `region`, in the cluster `cluster_id`, and answer
    /// its minted id — or refuse.
    ///
    /// The cluster the node goes in, the bound on the caller-chosen mapping
    /// list, the store's cap, the mint and the insert live together here, so a
    /// handler cannot check one of them and write past another; the handler's job
    /// is only to turn a [`NodeRefusal`] into the operation's own modelled error.
    ///
    /// The cluster has to EXIST, because a node is created "in the specified
    /// Cluster". Which state that cluster is in is not a precondition: the real
    /// service accepts a node in a cluster that has not finished coming up — the
    /// node is what has to register before either of them encodes anything — so a
    /// `CREATING` cluster takes nodes exactly as an `ACTIVE` one does.
    ///
    /// A created node starts `CREATED` and owes the clock NOTHING: the real
    /// service answers `CreateNode` with a node in that state and it is the
    /// REGISTRATION SCRIPT run on the node's own hardware that moves it on from
    /// there. This emulator runs no node hardware, so a created node stays
    /// `CREATED` rather than being walked to a `READY` it never reached.
    ///
    /// A `Name` a live node of that cluster already holds is REFUSED, because a
    /// node's name is unique in the Cluster ([`Self::node_named`]) however the
    /// node came to carry it: the create is held to the same rule the update is,
    /// so the store cannot come to hold two nodes of one cluster under one name.
    pub fn insert_node(
        &mut self,
        region: &str,
        cluster_id: &str,
        name: Option<String>,
        role: Option<String>,
        interface_mappings: Vec<NodeInterfaceMappingCreateRequest>,
        tags: HashMap<String, String>,
    ) -> Result<String, NodeRefusal> {
        // Resolved through the LIVE cluster: a cluster whose delete has been
        // accepted is on its way out of the account, so it takes no new node
        // even though its record stays readable.
        if self.live_cluster(cluster_id).is_none() {
            return Err(NodeRefusal::ClusterNotFound);
        }
        if interface_mappings.len() > MAX_NODE_INTERFACE_MAPPINGS {
            return Err(NodeRefusal::TooManyInterfaceMappings(
                interface_mappings.len(),
            ));
        }
        if self.nodes.len() >= MAX_NODES {
            return Err(NodeRefusal::StoreFull);
        }
        // The uniqueness of a node's name in its cluster, checked here as the
        // update checks it ([`Self::apply_node_changes`]): a create that asked
        // for a name a live node of this cluster holds stores nothing. A create
        // that names no name collides with nothing — an absent name is not a
        // name two nodes share.
        if let Some(name) = name.as_deref() {
            if self.node_named(cluster_id, name).is_some() {
                return Err(NodeRefusal::NameTaken);
            }
        }

        let id = self.mint_node_id();
        let arn = ::engine::emulator_arn("medialive", region, &format!("node:{id}"));
        self.nodes.insert(
            id.clone(),
            NodeRecord {
                id: id.clone(),
                arn,
                cluster_id: cluster_id.to_string(),
                name,
                role,
                node_interface_mappings: interface_mappings
                    .into_iter()
                    .map(described_interface_mapping)
                    .collect(),
                sdi_source_mappings: Vec::new(),
                tags,
                state: node_state::CREATED.to_string(),
                transition: None,
            },
        );
        Ok(id)
    }

    /// Record `token` as having created the node `node_id` in `cluster_id`,
    /// evicting the oldest token first if that would push the map past
    /// [`MAX_NODE_TOKENS`].
    pub fn record_node_token(&mut self, cluster_id: &str, token: String, node_id: &str) {
        self.node_tokens
            .record(node_token_key(cluster_id, &token), node_id, MAX_NODE_TOKENS);
    }

    /// The node a stored `RequestId` created in `cluster_id`, if that node is
    /// still one a caller may use.
    ///
    /// The cluster is part of the key AND re-checked on the record: a token
    /// presented to one cluster's endpoint says nothing about another's.
    ///
    /// LIVENESS is part of the lookup, through
    /// [`NodeRecord::is_deregistered`]: the delete keeps the record readable in
    /// the terminal `DEREGISTERED` state, but a token's idempotency window is
    /// over once the node it minted has reached that state — the name it carried
    /// is already free for a new node — so the retried create mints one of its
    /// own rather than answering with a node the caller could neither use nor
    /// change.
    pub fn node_for_token(&self, cluster_id: &str, request_id: &str) -> Option<&str> {
        let id = self.node_tokens.created(&node_token_key(cluster_id, request_id))?;
        self.node(cluster_id, id)
            .filter(|node| !node.is_deregistered())
            .map(|node| node.id.as_str())
    }

    /// The node `node_id` of the cluster `cluster_id`, whatever state it is in.
    ///
    /// Both ids have to match: `Id` is "Unique in the Cluster", so a node id
    /// addressed under a cluster it does not belong to names no node at all.
    pub fn node(&self, cluster_id: &str, node_id: &str) -> Option<&NodeRecord> {
        self.nodes
            .get(node_id)
            .filter(|node| node.cluster_id == cluster_id)
    }

    /// The same node, to write through — the one place a node is resolved
    /// mutably, so the "both ids have to match" rule of [`Self::node`] is stated
    /// once however many mutations the slice grows.
    fn node_mut(&mut self, cluster_id: &str, node_id: &str) -> Option<&mut NodeRecord> {
        self.nodes
            .get_mut(node_id)
            .filter(|node| node.cluster_id == cluster_id)
    }

    /// The ids of the channel placement groups this node is associated with —
    /// "An array of IDs. Each ID is one ChannelPlacementGroup that is associated
    /// with this Node. Empty if the Node is not yet associated with any groups".
    ///
    /// DERIVED on every read from the channel-placement-group slice beside this
    /// one, never recorded on the node: `CreateChannelPlacementGroup` is what
    /// associates a group with a node (its `Nodes`), so a group created or
    /// deleted after this node changes what the node reports. A group whose
    /// delete has been accepted is left out: it is on its way out of the
    /// account, so it associates nothing. Only the node's OWN cluster is walked
    /// — "The Node and the ChannelPlacementGroup must be in the same Cluster".
    ///
    /// The walk is in group-id order, because the store is a `BTreeMap`, so two
    /// reads report the list the same way round.
    fn channel_placement_groups_with_node(&self, cluster_id: &str, node_id: &str) -> Vec<String> {
        self.channel_placement_groups
            .values()
            .filter(|group| {
                group.cluster_id == cluster_id
                    && !group.is_delete_accepted()
                    && group.nodes.iter().any(|node| node == node_id)
            })
            .map(|group| group.id.clone())
            .collect()
    }

    /// The `DescribeNodeSummary` projection of one stored node — the model's one
    /// node shape, which `CreateNode`, `DescribeNode`, `DeleteNode` and
    /// `ListNodes` all report the members of.
    ///
    /// Every member is read off the stored record or derived from it, so the
    /// create, the read that follows it and the delete cannot report the same
    /// stored node differently.
    pub fn node_summary(&self, node: &NodeRecord) -> DescribeNodeSummary {
        DescribeNodeSummary {
            arn: Some(node.arn.clone()),
            // "An array of IDs. Each ID is one ChannelPlacementGroup that is
            // associated with this Node. Empty if the Node is not yet associated
            // with any groups": derived from the groups this store holds
            // ([`Self::channel_placement_groups_with_node`]), so it is empty
            // exactly while no live group of the node's cluster names it.
            channel_placement_groups: Some(
                self.channel_placement_groups_with_node(&node.cluster_id, &node.id),
            ),
            cluster_id: Some(node.cluster_id.clone()),
            connection_state: Some(NODE_CONNECTION_STATE.to_string()),
            id: Some(node.id.clone()),
            // ABSENT, not empty: `InstanceArn` and `ManagedInstanceId` name the
            // EC2 instance and the managed instance the real service registers
            // for a node's hardware, which this emulator does not provision. A
            // value here would claim an instance that does not exist.
            instance_arn: None,
            managed_instance_id: None,
            name: node.name.clone(),
            node_interface_mappings: Some(node.node_interface_mappings.clone()),
            role: node.role.clone(),
            // "The current state of the Node": the state the node is in as the
            // store sees it, which is the record's own except while work is
            // running on the node ([`Self::reported_node_state`]).
            state: Some(self.reported_node_state(node).to_string()),
            sdi_source_mappings: Some(node.sdi_source_mappings.clone()),
        }
    }

    /// The `State` a node REPORTS — "The current state of the Node"
    /// (`smithy_query(query="doc", shape="DescribeNodeSummary",
    /// member="State")`).
    ///
    /// A node whose deregistration has been accepted reports that: the
    /// `DEREGISTERING` it was accepted into and the terminal `DEREGISTERED` the
    /// clock settles it to are where the node is on its way out of the cluster,
    /// and nothing may be running on it from there.
    ///
    /// Otherwise `IN_USE` while a channel is running on the node
    /// ([`Self::channels_running_on_node`]) — that is the one `NodeState` value
    /// for a node a channel is using, and it is a fact about the service's own
    /// work rather than something a caller sets: `UpdateNodeState` asks whether
    /// MediaLive Anywhere MAY use the node (`ACTIVE` "to indicate that the Node
    /// is deployable", `DRAINING` "to isolate the Node so that MediaLive
    /// Anywhere won't use it"), not whether it currently IS using it, so a node
    /// the caller has drained still reports the work it has not finished yet.
    ///
    /// Otherwise the state the record carries: where the create left it
    /// ([`node_state::CREATED`]) or the state a caller last asked for. Derived
    /// on every read and never written, so a read reports what a later caller
    /// would see too.
    fn reported_node_state<'a>(&self, node: &'a NodeRecord) -> &'a str {
        if node.is_delete_accepted() {
            return &node.state;
        }
        if self
            .channels_running_on_node(&node.cluster_id, &node.id)
            .is_empty()
        {
            &node.state
        } else {
            node_state::IN_USE
        }
    }

    /// The `DescribeNodeSummary` of the node `node_id` in `cluster_id`, or `None`
    /// when the store holds no such node.
    pub fn node_description(&self, cluster_id: &str, node_id: &str) -> Option<DescribeNodeSummary> {
        let node = self.node(cluster_id, node_id)?;
        Some(self.node_summary(node))
    }

    /// The ids of the channels this node is RUNNING right now — what makes a
    /// node NOT idle, and what the refusal of its delete reports.
    ///
    /// A channel reaches a node through the ChannelPlacementGroup it is placed
    /// in: the group is assigned to the nodes it names
    /// ([`Self::channel_placement_groups_with_node`]) and a channel names the
    /// group it is placed in, so the channels on this node are the ones placed
    /// in those groups. Of those, only the ones whose pipelines are UP occupy
    /// the node ([`super::ChannelRecord::is_on_air`]): a channel that is
    /// waiting idle, being created, having its configuration applied or on its
    /// way out of the account runs nothing on the hardware, so it leaves the
    /// node idle.
    ///
    /// Derived from the store on every call, so it tracks every later start,
    /// stop, placement and delete rather than being recorded on the node.
    ///
    /// BOUNDED work: the group scan and the channel scan are each bounded by
    /// their own store's ceiling ([`super::MAX_CHANNEL_PLACEMENT_GROUPS`],
    /// [`super::MAX_CHANNELS`]).
    pub fn channels_running_on_node(&self, cluster_id: &str, node_id: &str) -> Vec<String> {
        self.channel_placement_groups_with_node(cluster_id, node_id)
            .iter()
            .flat_map(|group| self.channels_in_channel_placement_group(cluster_id, group))
            .filter(|channel| {
                self.channels
                    .get(channel)
                    .is_some_and(super::ChannelRecord::is_on_air)
            })
            .collect()
    }

    /// Accept a delete for the node `node_id` of `cluster_id` and answer the node
    /// as the acceptance left it, or refuse.
    ///
    /// "Delete a Node. The Node must be IDLE" is the documented precondition: a
    /// node that is still running channels ([`Self::channels_running_on_node`])
    /// is refused, exactly as a cluster that still holds resources refuses its
    /// own identically-worded precondition ([`Self::delete_cluster`]), and the
    /// refusal is taken BEFORE the first write, so a refused delete leaves the
    /// stored node exactly as it was.
    ///
    /// Which state the node is in is NOT a precondition of its own. The two
    /// states a caller can ask for are `ACTIVE` and `DRAINING`
    /// (`UpdateNodeStateShape`), and `DRAINING` is documented as the state that
    /// isolates the node "so that MediaLive Anywhere won't use it" — the state
    /// a node is put into on its way out of the cluster — so a drained node is
    /// idle rather than busy. What makes a node busy is work running on it, and
    /// that is what this checks.
    ///
    /// IDEMPOTENT: a delete already accepted writes nothing and is accepted
    /// again — the state the caller is asking for is the one the node is
    /// already heading for or in, so nothing about the request conflicts with
    /// the stored node, a repeat can neither restart the clock nor revive a
    /// deregistered node, and the idle rule is not applied a second time.
    ///
    /// The state write and the projection live together here, so the response
    /// reports the state that was stored rather than one a handler assembled: its
    /// `State` is `DEREGISTERING` as of this call (or the terminal
    /// `DEREGISTERED` on a repeat whose clock has already run) and every other
    /// member is the stored record's.
    pub fn deregister_node_description(
        &mut self,
        cluster_id: &str,
        node_id: &str,
        now: SystemTime,
    ) -> Result<DescribeNodeSummary, NodeDeleteRefusal> {
        let accepted = self
            .node(cluster_id, node_id)
            .ok_or(NodeDeleteRefusal::NoSuchNode)?
            .is_delete_accepted();
        if !accepted {
            let channels = self.channels_running_on_node(cluster_id, node_id);
            if !channels.is_empty() {
                return Err(NodeDeleteRefusal::NotIdle { channels });
            }
            self.node_mut(cluster_id, node_id)
                .ok_or(NodeDeleteRefusal::NoSuchNode)?
                .begin_deregister(now);
        }
        self.node_description(cluster_id, node_id)
            .ok_or(NodeDeleteRefusal::NoSuchNode)
    }

    /// Put the node `node_id` of `cluster_id` into the state `state` asks for
    /// and answer the node as that write left it — or refuse.
    ///
    /// `state` is `UpdateNodeState`'s own `State` member: `None` when the
    /// request carried none, which changes NOTHING (the model marks the member
    /// optional and documents no default, so a request that asks for no state
    /// asks for no change) and still reports the stored node. A value it does
    /// carry is one of `UpdateNodeStateShape`'s, refused at the request boundary
    /// when it is not, and is stored verbatim because every value that shape
    /// accepts is a `NodeState` the node can be in.
    ///
    /// The lookup and the write live together here, so the response reports the
    /// state that was STORED rather than the one the request asked for, and a
    /// node whose deregistration has already been accepted is refused rather
    /// than written: an id whose delete the service accepted names no node a
    /// caller may still change.
    pub fn apply_node_state(
        &mut self,
        cluster_id: &str,
        node_id: &str,
        state: Option<&str>,
    ) -> Result<DescribeNodeSummary, NodeStateRefusal> {
        let node = self
            .node_mut(cluster_id, node_id)
            .ok_or(NodeStateRefusal::NoSuchNode)?;
        if node.is_delete_accepted() {
            return Err(NodeStateRefusal::DeleteAccepted);
        }
        if let Some(state) = state {
            node.set_requested_state(state);
        }
        // Projected from the record this call just wrote, so the only outcomes
        // are the two refusals above and the node as the write left it: there is
        // no second lookup that could miss. The clone is what lets the summary
        // (which reads the rest of the store for the derived
        // `ChannelPlacementGroups`) be taken while the record itself is no
        // longer borrowed.
        let written = node.clone();
        Ok(self.node_summary(&written))
    }

    /// The live node of `cluster_id` called `name`, if that cluster holds one.
    ///
    /// `UpdateNode`'s own `Name` is documented "Specify a name that is unique in
    /// the Cluster … Names are case-sensitive", so at most one node answers and
    /// the comparison is exact. A node whose deregistration has been accepted
    /// reserves its name no longer: it is on its way out of the account, so the
    /// name it carried is free for another node, exactly as the channel
    /// placement group slice treats a group that is leaving.
    ///
    /// BOTH writes resolve against this. The sentence is written on
    /// `UpdateNodeRequest$Name`, but it states a property of the NODE rather than
    /// of one request — `CreateNodeRequest$Name` is "The user-specified name of
    /// the Node to be created" (`smithy_query(query="doc",
    /// shape="CreateNodeRequest", member="Name")`), which is the same name — so
    /// [`Self::insert_node`] refuses a clash exactly as
    /// [`Self::apply_node_changes`] does, the way every other named resource of
    /// this service enforces its own uniqueness on the create as well as on the
    /// update. That is also what keeps this lookup unambiguous: at most one live
    /// node of a cluster can ever hold a given name, so the `find` has at most
    /// one candidate to answer with.
    fn node_named(&self, cluster_id: &str, name: &str) -> Option<&NodeRecord> {
        self.nodes.values().find(|node| {
            node.cluster_id == cluster_id
                && !node.is_delete_accepted()
                && node.name.as_deref() == Some(name)
        })
    }

    /// The ids of the nodes whose `SdiSourceMappings` name the SdiSource `id` —
    /// the half of `DeleteSdiSource`'s documented precondition that reads the
    /// NODE store ("The SdiSource must not be part of any SidSourceMapping"),
    /// beside the half that reads the input store
    /// ([`Self::inputs_using_sdi_source`]).
    ///
    /// Derived from the stored nodes on every call, in node-id order (the store
    /// is ordered by id), so it tracks every later `UpdateNode` rather than
    /// being recorded on the source: a mapping is created and replaced by
    /// updating a node, so the node's current list is the only place the
    /// reference lives.
    ///
    /// A node whose deregistration has been accepted is left out — the one cut
    /// point every derived "currently using" list of this store applies
    /// ([`Self::inputs_using_sdi_source`],
    /// [`Self::channel_placement_groups_with_node`]): such a node is on its way
    /// out of the cluster, and [`Self::apply_node_changes`] refuses it, so
    /// nothing can send its mappings back in.
    pub(super) fn nodes_mapping_sdi_source(&self, id: &str) -> Vec<String> {
        self.nodes
            .values()
            .filter(|node| !node.is_delete_accepted())
            .filter(|node| {
                node.sdi_source_mappings
                    .iter()
                    .any(|mapping| mapping.sdi_source.as_deref() == Some(id))
            })
            .map(|node| node.id.clone())
            .collect()
    }

    /// One `SdiSourceMappings` request list, checked against what the model
    /// documents of it and projected onto the response shape — or refused.
    ///
    /// Each entry has to carry all three of its members ("You must specify all
    /// three parameters in this object"), name a card the hardware numbering
    /// reaches ([`FIRST_SDI_CARD_NUMBER`]) and name an SdiSource this account
    /// holds ("The ID of the SdiSource to associate with this port on this
    /// card. You can use the ListSdiSources operation to discover all the IDs"),
    /// which is the same resolution an input's own `SdiSources` list gets
    /// ([`ComAmazonawsMedialiveMedialiveState::input_configuration_error`]): a
    /// deleted source is held by the account no longer, so it is nothing a new
    /// mapping can connect a port to.
    ///
    /// The whole list is checked before the caller writes any of it, so a
    /// refused update leaves the stored node exactly as it was.
    fn described_sdi_source_mappings(
        &self,
        requested: Vec<SdiSourceMappingUpdateRequest>,
    ) -> Result<Vec<SdiSourceMapping>, NodeUpdateRefusal> {
        if requested.len() > MAX_NODE_SDI_SOURCE_MAPPINGS {
            return Err(NodeUpdateRefusal::TooManySdiSourceMappings(requested.len()));
        }
        let mut mappings = Vec::with_capacity(requested.len());
        for mapping in requested {
            let (Some(card_number), Some(channel_number), Some(sdi_source)) = (
                mapping.card_number,
                mapping.channel_number,
                mapping.sdi_source,
            ) else {
                return Err(NodeUpdateRefusal::IncompleteSdiSourceMapping);
            };
            if card_number < FIRST_SDI_CARD_NUMBER {
                return Err(NodeUpdateRefusal::CardNumberOutOfRange(card_number));
            }
            if self
                .sdi_sources
                .get(&sdi_source)
                .is_none_or(SdiSourceRecord::is_deleted)
            {
                // Whatever the member carried, including the empty string:
                // the account holds no source of that id, which is exactly
                // what this refusal says.
                return Err(NodeUpdateRefusal::UnknownSdiSource(sdi_source));
            }
            mappings.push(SdiSourceMapping {
                card_number: Some(card_number),
                channel_number: Some(channel_number),
                sdi_source: Some(sdi_source),
            });
        }
        Ok(mappings)
    }

    /// Apply an `UpdateNode` request's changes to the node the cluster/node pair
    /// names and answer the node as that write left it — or refuse.
    ///
    /// Every refusal is checked BEFORE the first write, so a refused update
    /// leaves the stored node exactly as it was. A member the request omitted is
    /// not changed ("Include this parameter only if you want to change the
    /// current …"), and a `SdiSourceMappings` the request DID carry replaces the
    /// node's mappings, because the member is the node's whole mapping list
    /// rather than an addition to it.
    ///
    /// A node whose deregistration has already been accepted is refused rather
    /// than written, exactly as [`Self::apply_node_state`] refuses it: an id
    /// whose delete the service accepted names no node a caller may still
    /// change. Nothing here owes the clock a step — a node's settings are in
    /// effect as of this call and its `State` is not one of them.
    pub fn apply_node_changes(
        &mut self,
        cluster_id: &str,
        node_id: &str,
        changes: NodeChanges,
    ) -> Result<DescribeNodeSummary, NodeUpdateRefusal> {
        match self.node(cluster_id, node_id) {
            None => return Err(NodeUpdateRefusal::NoSuchNode),
            Some(node) if node.is_delete_accepted() => {
                return Err(NodeUpdateRefusal::DeleteAccepted)
            }
            Some(_) => {}
        }
        // A name another live node of this cluster holds is refused, while
        // re-sending the node's own name is not a collision with itself.
        if let Some(name) = changes.name.as_deref() {
            if self
                .node_named(cluster_id, name)
                .is_some_and(|holder| holder.id != node_id)
            {
                return Err(NodeUpdateRefusal::NameTaken);
            }
        }
        let mappings = match changes.sdi_source_mappings {
            Some(requested) => Some(self.described_sdi_source_mappings(requested)?),
            None => None,
        };

        let node = self
            .node_mut(cluster_id, node_id)
            // UNREACHABLE: the node was resolved at the top of this same
            // exclusive borrow and nothing between the two lookups touches the
            // store, so this is the invariant restated rather than a second
            // failure mode.
            .ok_or(NodeUpdateRefusal::NoSuchNode)?;
        if let Some(name) = changes.name {
            node.name = Some(name);
        }
        if let Some(role) = changes.role {
            node.role = Some(role);
        }
        if let Some(mappings) = mappings {
            node.sdi_source_mappings = mappings;
        }
        // Projected from the record this call just wrote, for the same reason
        // [`Self::apply_node_state`] projects its own write.
        let written = node.clone();
        Ok(self.node_summary(&written))
    }
}

/// The changes one `UpdateNode` request asks for: the members it carried.
///
/// `None` is "the request named this member not at all", which asks for no
/// change to it — every member of the request shape is optional and the model
/// documents no default for any of them.
#[derive(Debug, Clone, Default)]
pub struct NodeChanges {
    /// The `Name` to change the node's to, which has to be unique in the
    /// cluster the node belongs to.
    pub name: Option<String>,
    /// The `NodeRole` to change the node's to, stored verbatim: the member
    /// targets an enum, so a value outside it is refused at the request
    /// boundary.
    pub role: Option<String>,
    /// The node's whole SDI source mapping list, as the request shape carries
    /// it.
    pub sdi_source_mappings: Option<Vec<SdiSourceMappingUpdateRequest>>,
}

/// One created node's interface mapping, projected from the CREATE REQUEST shape
/// onto the response shape.
///
/// The response shape carries one member the request does not —
/// `PhysicalInterfaceIpAddresses`, "The IP addresses associated with the physical
/// interface on the node hardware" — which the real service learns from the
/// hardware when the node registers. This emulator runs no node hardware, so the
/// member is ABSENT rather than an empty list that would claim an interface with
/// no addresses.
fn described_interface_mapping(mapping: NodeInterfaceMappingCreateRequest) -> NodeInterfaceMapping {
    NodeInterfaceMapping {
        logical_interface_name: mapping.logical_interface_name,
        network_interface_mode: mapping.network_interface_mode,
        physical_interface_name: mapping.physical_interface_name,
        physical_interface_ip_addresses: None,
    }
}

/// The key the node idempotency-token map records a token against.
///
/// A node is created against ONE cluster's endpoint (`POST
/// /prod/clusters/{ClusterId}/nodes`) and its id is unique in that cluster, so
/// the cluster is part of the key: two clusters using the same `RequestId` each
/// keep their own entry instead of overwriting one another's. The cluster is
/// re-checked on the record the token resolves to
/// ([`ComAmazonawsMedialiveMedialiveState::node_for_token`]), so the key's shape
/// carries no invariant of its own.
fn node_token_key(cluster_id: &str, token: &str) -> String {
    format!("{cluster_id}/{token}")
}

/// Project a `DescribeNodeSummary` onto one of the structurally identical
/// per-operation flat node output shapes (`CreateNodeOutput`,
/// `DescribeNodeOutput`, `DeleteNodeOutput`, `UpdateNodeStateOutput` and
/// `UpdateNodeOutput`).
///
/// One macro rather than a hand-written copy of the same member move per
/// operation, so a create and the read that follows it cannot report the same
/// stored node differently. `ManagedInstanceId` is the one member the summary
/// carries that the flat shapes do not model, so it is dropped here rather than
/// named in each of them.
macro_rules! node_description_output {
    ($out:ident, $node:expr) => {{
        let node: $crate::ops::types::DescribeNodeSummary = $node;
        $out {
            arn: node.arn,
            channel_placement_groups: node.channel_placement_groups,
            cluster_id: node.cluster_id,
            connection_state: node.connection_state,
            id: node.id,
            instance_arn: node.instance_arn,
            name: node.name,
            node_interface_mappings: node.node_interface_mappings,
            role: node.role,
            state: node.state,
            sdi_source_mappings: node.sdi_source_mappings,
        }
    }};
}

pub(crate) use node_description_output;
