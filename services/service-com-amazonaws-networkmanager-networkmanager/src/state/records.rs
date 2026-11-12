// SPDX-License-Identifier: CC0-1.0

//! The stored records of the global network and core network inventory — global
//! networks, sites, links, devices, their associations, connections, core networks and
//! their policy versions — and the modelled shapes a read of one answers.

use super::*;

/// A stored global network. `state` is not a field: it is a function of the clock
/// (see [`GlobalNetworkRecord::state_at`]), so reading one never writes.
#[derive(Debug, Clone)]
pub struct GlobalNetworkRecord {
    pub id: String,
    pub arn: String,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured the global network and `None` while no update
    /// has ever touched it. See [`GlobalNetworkRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl GlobalNetworkRecord {
    /// This network's lifecycle state as of `now`.
    ///
    /// Both transitions are a function of the clock, so reading one never writes: a global
    /// network is `UPDATING` from an update until that reconfiguration is applied,
    /// `PENDING` until the create settles, and `AVAILABLE` once neither is outstanding. An
    /// update OUTSTANDING is what it reports even when the create has not settled yet,
    /// since that update is the later of the two requests it is still working on — the rule
    /// [`LinkRecord::state_at`] states for every record of this service whose state enum
    /// carries `UPDATING`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
        } else {
            STATE_AVAILABLE
        }
    }

    /// The modelled `GlobalNetwork` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> GlobalNetwork {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `GlobalNetwork` for this record, forcing `state` — used by
    /// `DeleteGlobalNetwork`, which reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> GlobalNetwork {
        GlobalNetwork {
            global_network_id: Some(self.id.clone()),
            global_network_arn: Some(self.arn.clone()),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored site, back-referencing the global network that owns it.
#[derive(Debug, Clone)]
pub struct SiteRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub description: Option<String>,
    pub location: Option<Location>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured the site and `None` while no update has ever
    /// touched it. See [`SiteRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl SiteRecord {
    /// This site's lifecycle state as of `now`, by the rule [`LinkRecord::state_at`]
    /// states: `UPDATING` while an update is outstanding, `PENDING` until the create
    /// settles, and `AVAILABLE` once neither is.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
        } else {
            STATE_AVAILABLE
        }
    }

    /// The modelled `Site` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Site {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Site` for this record, forcing `state` — used by `DeleteSite`.
    pub fn to_shape_with_state(&self, state: &str) -> Site {
        Site {
            site_id: Some(self.id.clone()),
            site_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            description: self.description.clone(),
            location: self.location.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored link, back-referencing the site (and through it the global network) it
/// belongs to. Like the records above, `state` is a function of the clock.
#[derive(Debug, Clone)]
pub struct LinkRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub site_id: String,
    pub description: Option<String>,
    /// The link's `Type`, a free-form string the model does not enumerate.
    pub link_type: Option<String>,
    /// The upload/download speeds in Mbps, as the request supplied them.
    pub bandwidth: Bandwidth,
    pub provider: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured the link and `None` while no update has ever
    /// touched it. See [`LinkRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl LinkRecord {
    /// This link's lifecycle state as of `now`.
    ///
    /// Both transitions are a function of the clock, so reading one never writes: a link
    /// is `UPDATING` from an update until that reconfiguration is applied, `PENDING` until
    /// the create settles, and `AVAILABLE` once neither is outstanding. An update
    /// OUTSTANDING is what the link reports even when the create has not settled yet,
    /// since that update is the later of the two requests the link is still working on.
    ///
    /// This is the rule EVERY record of this service whose state enum carries `UPDATING`
    /// follows ([`GlobalNetworkRecord::state_at`], [`SiteRecord::state_at`],
    /// [`DeviceRecord::state_at`], [`ConnectionRecord::state_at`],
    /// [`CoreNetworkRecord::state_at`], [`AttachmentRecord::state_at`]): an update of such
    /// a resource is not applied instantly, so the resource reports the non-terminal state
    /// its reconfiguration is outstanding in until the transition the clock owes falls due.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
        } else {
            STATE_AVAILABLE
        }
    }

    /// The modelled `Link` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Link {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Link` for this record, forcing `state` — used by `DeleteLink`, which
    /// reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> Link {
        Link {
            link_id: Some(self.id.clone()),
            link_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            site_id: Some(self.site_id.clone()),
            description: self.description.clone(),
            r#type: self.link_type.clone(),
            bandwidth: Some(self.bandwidth.clone()),
            provider: self.provider.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored device, back-referencing the global network that owns it and, when the
/// create named one, the site it sits at.
///
/// A device hangs off the GLOBAL NETWORK, not off a site: "Creates a new device in a
/// global network" (`smithy_query query="doc" shape="CreateDevice"`) and `SiteId` is
/// optional, so `site_id` is an `Option` where [`LinkRecord::site_id`] is required.
/// Like the records above, `state` is a function of the clock rather than a field.
#[derive(Debug, Clone)]
pub struct DeviceRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    /// The site this device sits at, when the create named one.
    pub site_id: Option<String>,
    /// The Amazon Web Services location (zone / subnet ARN) of the device, if any: "For
    /// an on-premises device, you can omit this parameter"
    /// (`smithy_query query="doc" shape="CreateDeviceRequest" member="AWSLocation"`).
    pub aws_location: Option<AWSLocation>,
    pub description: Option<String>,
    /// The device's `Type`, a free-form string the model does not enumerate.
    pub device_type: Option<String>,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub location: Option<Location>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured the device and `None` while no update has ever
    /// touched it. See [`DeviceRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl DeviceRecord {
    /// This device's lifecycle state as of `now`. `DeviceState` carries the same four
    /// values as the three enums above (`smithy_query shape=DeviceState`: PENDING,
    /// AVAILABLE, DELETING, UPDATING), so a device waits in [`STATE_PENDING`] too and
    /// reports [`STATE_UPDATING`] while a reconfiguration is outstanding, exactly as
    /// [`LinkRecord::state_at`] reports a link's.
    ///
    /// Both transitions are a function of the clock, so reading a device never writes, and
    /// an outstanding update is what it reports even when the create has not settled yet:
    /// that update is the later of the two requests it is still working on.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
        } else {
            STATE_AVAILABLE
        }
    }

    /// The modelled `Device` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Device {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Device` for this record, forcing `state` — for an operation that
    /// reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> Device {
        Device {
            device_id: Some(self.id.clone()),
            device_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            aws_location: self.aws_location.clone(),
            description: self.description.clone(),
            r#type: self.device_type.clone(),
            vendor: self.vendor.clone(),
            model: self.model.clone(),
            serial_number: self.serial_number.clone(),
            location: self.location.clone(),
            site_id: self.site_id.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored core network policy VERSION: one immutable document a core network holds.
///
/// "Creates a new, immutable version of a core network policy" (`smithy_query
/// query="doc" shape="PutCoreNetworkPolicy"`), so a stored version is never edited: a
/// later put mints the next version beside it. The document lives HERE and nowhere else
/// — a core network's LIVE document is the one of the version
/// [`CoreNetworkRecord::live_policy_version`] names — so no two records of this service
/// hold a copy of the same policy.
#[derive(Debug, Clone)]
pub struct CoreNetworkPolicyRecord {
    /// The core network this version belongs to, carried so a read of the version reports
    /// it without the reader having to pass it back in.
    pub core_network_id: String,
    pub version_id: i32,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// The instant the change-set transition the clock owes this version becomes due:
    /// `PENDING_GENERATION` -> `READY_TO_EXECUTE` while it waits to be executed, or
    /// `EXECUTING` -> `EXECUTION_SUCCEEDED` while its document is being put in force.
    pub settled_at: SystemTime,
    /// The policy document, verbatim as the request supplied it.
    pub document: String,
    /// The topology [`policy_topology`] derived from [`Self::document`], derived ONCE where
    /// that document was accepted — before the store was locked, by the request that
    /// carried it — and kept here beside it.
    ///
    /// A version's document never changes, so neither does this: it is what the core
    /// network reports while this version is the LIVE one
    /// ([`CoreNetworkRecord::topology`]), the submitted side of this version's own change
    /// set, and what a restore of this version carries to the version it mints. Nothing
    /// re-parses the document to get it, so no operation of this service parses a policy
    /// document while holding the store lock. What it holds is bounded by the caps
    /// [`validate_policy_document`] put on the document before it was stored — the topology
    /// is linear in the document it came from — and a version is itself bounded by
    /// [`MAX_POLICY_VERSIONS_PER_CORE_NETWORK`].
    pub topology: PolicyTopology,
    /// The `ClientToken` of the `PutCoreNetworkPolicy` that minted this version, kept so a
    /// retry of that request answers this version instead of minting a second one.
    pub client_token: Option<String>,
    /// The instant the execution of this version's change set began — which is what puts
    /// its document in force — and `None` for a version whose change set has not been
    /// executed. The version a create carries is executed by the create itself ("A
    /// subsequent change set is created showing the differences between the LIVE policy and
    /// the submitted policy", `smithy_query query="doc" shape="PutCoreNetworkPolicy"` — a
    /// put only PREPARES one); the change set a put or a restore prepared is executed by
    /// `ExecuteCoreNetworkChangeSet` ([`CoreNetworkRecord::execute_policy_version`]).
    pub executed_at: Option<SystemTime>,
    /// Whether this version's change set describes differences from a document that is no
    /// longer in force, which is what makes it [`CHANGE_SET_OUT_OF_DATE`].
    ///
    /// A change set is generated ONCE, against the document in force at that moment, so
    /// putting ANOTHER version in force leaves every change set still waiting to be
    /// executed describing a comparison that no longer holds — it is the stored flag rather
    /// than a function of the clock because what invalidates it is that later execution,
    /// not the passing of time. An executed version is unaffected: its change set has
    /// already been applied.
    pub out_of_date: bool,
    /// The changes this version's change set carries: the difference between the policy
    /// that was in force when this version was stored and this version's own document
    /// ([`policy_changes`]).
    ///
    /// Computed ONCE, where the version is stored, because that is when the change set is
    /// generated — "a subsequent change set is created showing the differences between the
    /// LIVE policy and the submitted policy" (`smithy_query query="doc"
    /// shape="PutCoreNetworkPolicy"`) — so a later put that moves the LIVE policy does not
    /// rewrite the change set of a version already stored, and a read of these changes
    /// never re-parses a document. The list is bounded by the caps
    /// [`validate_policy_document`] puts on the two documents compared, and a version is
    /// itself bounded by [`MAX_POLICY_VERSIONS_PER_CORE_NETWORK`].
    pub changes: Vec<PolicyChange>,
}

impl CoreNetworkPolicyRecord {
    /// Whether this version's change set has been executed, which is what puts its
    /// document in force.
    pub fn executed(&self) -> bool {
        self.executed_at.is_some()
    }

    /// This version's `ChangeSetState` as of `now` — a function of the clock and of the
    /// stored record, so reading a version never writes. See
    /// [`CHANGE_SET_PENDING_GENERATION`].
    pub fn change_set_state_at(&self, now: SystemTime) -> &'static str {
        // A change set superseded by another version's execution can never be executed, so
        // it reports that instead of the state the clock would otherwise give it; only an
        // unexecuted one can be superseded.
        if !self.executed() && self.out_of_date {
            return CHANGE_SET_OUT_OF_DATE;
        }
        match (self.executed(), now >= self.settled_at) {
            (false, false) => CHANGE_SET_PENDING_GENERATION,
            (false, true) => CHANGE_SET_READY_TO_EXECUTE,
            (true, false) => CHANGE_SET_EXECUTING,
            (true, true) => CHANGE_SET_EXECUTION_SUCCEEDED,
        }
    }

    /// The change events of this version's change set as of `now`, each keyed by its
    /// `IdentifierPath` — in that key's order, which is the order
    /// [`policy_changes`] records the changes in.
    ///
    /// One event per change the change set carries, reporting the status that change's
    /// execution is in as of `now` and the instant it last entered that status: the version
    /// was stored (nothing executed yet), the execution began, or the execution succeeded.
    /// Both are functions of stored instants and the clock, so reading them writes nothing
    /// and two reads of an execution that has settled report the same events.
    pub fn change_events_at(&self, now: SystemTime) -> Vec<(String, CoreNetworkChangeEvent)> {
        let (status, event_time) = match self.executed_at {
            Some(_) if now >= self.settled_at => (CHANGE_STATUS_COMPLETE, self.settled_at),
            Some(started_at) => (CHANGE_STATUS_IN_PROGRESS, started_at),
            None => (CHANGE_STATUS_NOT_STARTED, self.created_at),
        };
        self.changes
            .iter()
            .map(|change| {
                (
                    change.identifier_path(),
                    change.to_change_event(status, event_time),
                )
            })
            .collect()
    }

    /// The changes of this version's change set, each keyed by its `IdentifierPath` — in
    /// that key's order, which is the order [`policy_changes`] records the changes in.
    ///
    /// The SAME stored changes [`Self::change_events_at`] reports as the events of this
    /// version's execution, reported instead as the change set itself: what each change
    /// does to the object it names, with the values that object held under the policy this
    /// version was submitted against and the ones this version's document gives it. Unlike
    /// an event, a change carries no status and no instant, so this answer is independent
    /// of the clock: the change set is a property of the version, not of its execution.
    pub fn change_set(&self) -> Vec<(String, CoreNetworkChange)> {
        self.changes
            .iter()
            .map(|change| (change.identifier_path(), change.to_core_network_change()))
            .collect()
    }

    /// The modelled `CoreNetworkPolicy` for this version, as of `now`, carrying the
    /// `alias` its core network gives it ([`CoreNetworkRecord::policy_alias_of`]).
    pub fn to_shape(&self, alias: Option<&str>, now: SystemTime) -> CoreNetworkPolicy {
        CoreNetworkPolicy {
            core_network_id: Some(self.core_network_id.clone()),
            policy_version_id: Some(self.version_id),
            alias: alias.map(str::to_string),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            change_set_state: Some(self.change_set_state_at(now).to_string()),
            policy_errors: no_reported_errors(),
            policy_document: Some(self.document.clone()),
        }
    }

    /// The modelled `CoreNetworkPolicyVersion` for this version, as of `now`, carrying the
    /// `alias` its core network gives it ([`CoreNetworkRecord::policy_alias_of`]).
    ///
    /// The SUMMARY of the version a listing reports: the same members as
    /// [`Self::to_shape`] without the document and its errors (`smithy_query
    /// shape=CoreNetworkPolicyVersion`), so a listing and a read of one version agree on
    /// everything they both report.
    pub fn to_version_shape(
        &self,
        alias: Option<&str>,
        now: SystemTime,
    ) -> CoreNetworkPolicyVersion {
        CoreNetworkPolicyVersion {
            core_network_id: Some(self.core_network_id.clone()),
            policy_version_id: Some(self.version_id),
            alias: alias.map(str::to_string),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            change_set_state: Some(self.change_set_state_at(now).to_string()),
        }
    }
}

/// A stored link association: the tie between one device and one link of the same global
/// network and the same site.
///
/// The association has no id and no ARN of its own — the modelled `LinkAssociation` is
/// the device/link pair plus its state (`smithy_query shape=LinkAssociation`) — so the
/// pair is its identity and the store keys it by [`link_association_key`]. Like the
/// records above, its state is a function of the clock rather than a field, so reading
/// one never writes.
#[derive(Debug, Clone)]
pub struct LinkAssociationRecord {
    pub global_network_id: String,
    pub device_id: String,
    pub link_id: String,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
}

impl LinkAssociationRecord {
    /// This association's state as of `now`.
    ///
    /// `LinkAssociationState` carries PENDING, AVAILABLE, DELETING and DELETED
    /// (`smithy_query shape=LinkAssociationState`, mirrored in
    /// `ops::types::LINK_ASSOCIATION_STATE_VALUES`); the first three are spelled exactly
    /// as [`STATE_PENDING`], [`STATE_AVAILABLE`] and [`STATE_DELETING`], so this slice
    /// reuses those constants.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `LinkAssociation` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> LinkAssociation {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `LinkAssociation` for this record, forcing `state` — used by
    /// `DisassociateLink`, which reports the association it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> LinkAssociation {
        LinkAssociation {
            global_network_id: Some(self.global_network_id.clone()),
            device_id: Some(self.device_id.clone()),
            link_id: Some(self.link_id.clone()),
            link_association_state: Some(state.to_string()),
        }
    }
}

/// The key the association store holds a device/link pair under: the device id, then the
/// link id, so the map orders associations by device and then by link and a pagination
/// token can name an offset into it.
///
/// An association is only ever stored for a device and a link this service minted ids for
/// ([`ComAmazonawsNetworkmanagerNetworkmanagerState::next_id`]), each `<kind>-<17 hex>`,
/// so no two pairs can produce the same key. The same string identifies the association
/// in a not-found refusal, where no single resource id would.
pub fn link_association_key(device_id: &str, link_id: &str) -> String {
    format!("{device_id}/{link_id}")
}

/// A stored customer gateway association: the tie between one customer gateway of Amazon
/// VPC and one device — and optionally one link — of a global network.
///
/// Like [`LinkAssociationRecord`] this is NOT a resource of its own: the modelled
/// `CustomerGatewayAssociation` carries no id and no ARN of its own (`smithy_query
/// shape=CustomerGatewayAssociation`), so the global network plus the customer gateway ARN
/// is its identity and the store keys it by [`customer_gateway_association_key`]. Its
/// state is a function of the clock rather than a field, so reading one never writes.
#[derive(Debug, Clone)]
pub struct CustomerGatewayAssociationRecord {
    pub global_network_id: String,
    /// The ARN of the customer gateway, as the request carried it: the gateway belongs to
    /// Amazon VPC, so this service stores the reference and holds no record of the gateway
    /// itself (see [`parse_customer_gateway_arn`]).
    pub customer_gateway_arn: String,
    pub device_id: String,
    /// The link the association names, or `None` when the associate did not name one —
    /// `LinkId` is the one optional member of the request (`smithy_query
    /// query="operation" operation="AssociateCustomerGateway"`).
    pub link_id: Option<String>,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
}

impl CustomerGatewayAssociationRecord {
    /// This association's state as of `now`.
    ///
    /// `CustomerGatewayAssociationState` carries PENDING, AVAILABLE, DELETING and DELETED
    /// (`smithy_query shape=CustomerGatewayAssociationState`, mirrored in
    /// `ops::types::CUSTOMER_GATEWAY_ASSOCIATION_STATE_VALUES`); the first three are
    /// spelled exactly as [`STATE_PENDING`], [`STATE_AVAILABLE`] and [`STATE_DELETING`],
    /// so this slice reuses those constants.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `CustomerGatewayAssociation` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> CustomerGatewayAssociation {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `CustomerGatewayAssociation` for this record, forcing `state` — used by
    /// `DisassociateCustomerGateway`, which reports the association it just removed as
    /// `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> CustomerGatewayAssociation {
        CustomerGatewayAssociation {
            customer_gateway_arn: Some(self.customer_gateway_arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            device_id: Some(self.device_id.clone()),
            link_id: self.link_id.clone(),
            state: Some(state.to_string()),
        }
    }
}

/// The key the store holds a customer gateway association under: the global network it was
/// made in, then the ARN of the customer gateway it ties to one of that network's devices.
///
/// The ARN alone is not the key: the association is addressed THROUGH the global network in
/// every one of the three operations' URIs (`smithy_query query="operation"
/// operation="AssociateCustomerGateway,DisassociateCustomerGateway,GetCustomerGatewayAssociations"`),
/// so a pair made in a DIFFERENT global network is not addressable here — exactly as
/// [`prefix_list_association_key`] scopes a prefix list to its core network. The same
/// string identifies the association in a not-found refusal, where no single resource id
/// would.
pub fn customer_gateway_association_key(global_network_id: &str, customer_gateway_arn: &str) -> String {
    format!("{global_network_id}/{customer_gateway_arn}")
}

/// A stored transit gateway registration: the tie between one transit gateway of Amazon VPC
/// and one global network.
///
/// Like [`CustomerGatewayAssociationRecord`] this is NOT a resource of its own: the modelled
/// `TransitGatewayRegistration` carries the global network, the gateway's ARN and a state
/// reason, with no id and no ARN of its own (`smithy_query
/// shape=TransitGatewayRegistration`), so the global network plus the gateway's ARN is its
/// identity and the store keys it by [`transit_gateway_registration_key`]. Its state is a
/// function of the clock rather than a field, so reading one never writes.
#[derive(Debug, Clone)]
pub struct TransitGatewayRegistrationRecord {
    pub global_network_id: String,
    /// The ARN of the transit gateway, as the request carried it: the gateway belongs to
    /// Amazon VPC, so this service stores the reference and holds no record of the gateway
    /// itself (see [`ec2_resource_region`]).
    pub transit_gateway_arn: String,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
}

impl TransitGatewayRegistrationRecord {
    /// This registration's state as of `now`.
    ///
    /// `TransitGatewayRegistrationState` carries PENDING, AVAILABLE, DELETING, DELETED and
    /// FAILED (`smithy_query shape=TransitGatewayRegistrationState`, mirrored in
    /// `ops::types::TRANSIT_GATEWAY_REGISTRATION_STATE_VALUES`); the first three are spelled
    /// exactly as [`STATE_PENDING`], [`STATE_AVAILABLE`] and [`STATE_DELETING`], so this
    /// slice reuses those constants. A stored registration is one of the first two: nothing
    /// here fails a registration, and a deregistered one is removed rather than kept.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `TransitGatewayRegistration` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> TransitGatewayRegistration {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `TransitGatewayRegistration` for this record, forcing `state` — used by
    /// `DeregisterTransitGateway`, which reports the registration it just removed as
    /// `DELETING`.
    ///
    /// `State$Message` is left absent: "The message for the state reason" (`smithy_query
    /// query="doc" shape="TransitGatewayRegistrationStateReason" member="Message"`) is what
    /// a service reporting WHY a registration reached its state fills in, and the two states
    /// a registration of this store reaches are the ordinary ones the code already names.
    pub fn to_shape_with_state(&self, state: &str) -> TransitGatewayRegistration {
        TransitGatewayRegistration {
            global_network_id: Some(self.global_network_id.clone()),
            transit_gateway_arn: Some(self.transit_gateway_arn.clone()),
            state: Some(TransitGatewayRegistrationStateReason {
                code: Some(state.to_string()),
                message: None,
            }),
        }
    }
}

/// The key the store holds a transit gateway registration under: the global network it was
/// made in, then the ARN of the transit gateway registered with it.
///
/// Like [`customer_gateway_association_key`], the ARN alone is not the key: the registration
/// is addressed THROUGH the global network in every one of the three operations' URIs
/// (`smithy_query query="operation"
/// operation="RegisterTransitGateway,DeregisterTransitGateway,GetTransitGatewayRegistrations"`).
/// The same string identifies the registration in a not-found refusal, where no single
/// resource id would.
pub fn transit_gateway_registration_key(
    global_network_id: &str,
    transit_gateway_arn: &str,
) -> String {
    format!("{global_network_id}/{transit_gateway_arn}")
}

/// A stored transit gateway Connect peer association: the tie between one Connect peer of a
/// transit gateway of Amazon VPC and one DEVICE of a global network, optionally over one of
/// that device's links.
///
/// "Associates a transit gateway Connect peer with a device, and optionally, with a link."
/// (`smithy_query query="doc" shape="AssociateTransitGatewayConnectPeer"`) \u2014 like
/// [`CustomerGatewayAssociationRecord`] the tie carries no id and no ARN of its own (the
/// modelled `TransitGatewayConnectPeerAssociation` is the peer's ARN, the global network,
/// the device, the link and a state: `smithy_query
/// shape=TransitGatewayConnectPeerAssociation`), so the global network plus the peer's ARN
/// is its identity and the store keys it by
/// [`transit_gateway_connect_peer_association_key`] \u2014 which is exactly what
/// `DisassociateTransitGatewayConnectPeer` addresses, naming those two and nothing else
/// (`smithy_query query="operation" operation="DisassociateTransitGatewayConnectPeer"`).
///
/// Like the records above, its state is a function of the clock rather than a field, so
/// reading one never writes.
#[derive(Debug, Clone)]
pub struct TransitGatewayConnectPeerAssociationRecord {
    pub global_network_id: String,
    /// The ARN of the transit gateway Connect peer, as the request carried it: the peer
    /// belongs to Amazon VPC, so this service stores the reference and holds no record of
    /// the peer itself (see [`ec2_resource_region`]).
    pub transit_gateway_connect_peer_arn: String,
    pub device_id: String,
    /// The link the device reaches this peering over, or `None` where the association named
    /// none \u2014 `LinkId` is the one optional member of the request.
    pub link_id: Option<String>,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
}

impl TransitGatewayConnectPeerAssociationRecord {
    /// This association's state as of `now`.
    ///
    /// `TransitGatewayConnectPeerAssociationState` carries PENDING, AVAILABLE, DELETING and
    /// DELETED (`smithy_query shape=TransitGatewayConnectPeerAssociationState`, mirrored in
    /// `ops::types::TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION_STATE_VALUES`); the first three
    /// are spelled exactly as [`STATE_PENDING`], [`STATE_AVAILABLE`] and [`STATE_DELETING`],
    /// so this slice reuses those constants, as [`CustomerGatewayAssociationRecord`] does.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `TransitGatewayConnectPeerAssociation` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> TransitGatewayConnectPeerAssociation {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `TransitGatewayConnectPeerAssociation` for this record, forcing `state`
    /// \u2014 used by `DisassociateTransitGatewayConnectPeer`, which reports the association it
    /// just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> TransitGatewayConnectPeerAssociation {
        TransitGatewayConnectPeerAssociation {
            transit_gateway_connect_peer_arn: Some(self.transit_gateway_connect_peer_arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            device_id: Some(self.device_id.clone()),
            link_id: self.link_id.clone(),
            state: Some(state.to_string()),
        }
    }
}

/// The key the store holds a transit gateway Connect peer association under: the global
/// network it was made in, then the ARN of the Connect peer tied to one of that network's
/// devices.
///
/// Like [`customer_gateway_association_key`], the ARN alone is not the key: the association
/// is addressed THROUGH the global network in every one of the three operations' URIs
/// (`smithy_query query="operation"
/// operation="AssociateTransitGatewayConnectPeer,DisassociateTransitGatewayConnectPeer,GetTransitGatewayConnectPeerAssociations"`),
/// so a tie made in a DIFFERENT global network is not addressable here. The same string
/// identifies the association in a not-found refusal, where no single resource id would.
pub fn transit_gateway_connect_peer_association_key(
    global_network_id: &str,
    transit_gateway_connect_peer_arn: &str,
) -> String {
    format!("{global_network_id}/{transit_gateway_connect_peer_arn}")
}

/// A stored connection: the tie between two devices of one global network.
///
/// "Creates a connection between two devices" (`smithy_query query="doc"
/// shape="CreateConnection"`), so a connection hangs off the GLOBAL NETWORK and names two
/// of its devices; the two link members are optional, each naming the link that device
/// reaches the connection over. Unlike a link association this one IS a resource: it has
/// an id, an ARN and tags of its own. Like every record above, its state is a function of
/// the clock rather than a field, so reading one never writes.
#[derive(Debug, Clone)]
pub struct ConnectionRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    /// The first device in the connection.
    pub device_id: String,
    /// The second device in the connection.
    pub connected_device_id: String,
    /// The link the first device reaches the connection over, when the request named one.
    pub link_id: Option<String>,
    /// The link the second device reaches the connection over, when the request named one.
    pub connected_link_id: Option<String>,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured the connection and `None` while no update has
    /// ever touched it. See [`ConnectionRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    pub tags: Vec<Tag>,
}

impl ConnectionRecord {
    /// This connection's lifecycle state as of `now`. `ConnectionState` carries the same
    /// four values as the enums above (`smithy_query shape=ConnectionState`: PENDING,
    /// AVAILABLE, DELETING, UPDATING, mirrored in `ops::types::CONNECTION_STATE_VALUES`),
    /// so a connection waits in [`STATE_PENDING`] too and reports [`STATE_UPDATING`] while
    /// a reconfiguration is outstanding, by the rule [`LinkRecord::state_at`] states.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else if now < self.available_at {
            STATE_PENDING
        } else {
            STATE_AVAILABLE
        }
    }

    /// The modelled `Connection` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Connection {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Connection` for this record, forcing `state` — for an operation that
    /// reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> Connection {
        Connection {
            connection_id: Some(self.id.clone()),
            connection_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            device_id: Some(self.device_id.clone()),
            connected_device_id: Some(self.connected_device_id.clone()),
            link_id: self.link_id.clone(),
            connected_link_id: self.connected_link_id.clone(),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored core network: the Cloud WAN managed network a global network owns.
///
/// Its `state` is a function of the clock, like the records above, and its topology
/// (`Segments`, `NetworkFunctionGroups`, `Edges`) is the one derived from the LIVE policy
/// version's document where that document was accepted
/// ([`CoreNetworkPolicyRecord::topology`], reached through [`Self::topology`]) — nothing
/// about a core network is kept twice and no read re-parses the document.
#[derive(Debug, Clone)]
pub struct CoreNetworkRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// Every policy version this core network holds, by `PolicyVersionId`. The documents
    /// live in these records alone, so a read of a version reports what was put, and the
    /// LIVE document is the document of the version [`Self::live_policy_version`] names.
    pub policy_versions: BTreeMap<i32, CoreNetworkPolicyRecord>,
    /// The version whose document is in force (the one aliased `LIVE`), or `None` for a
    /// core network created without a policy document.
    pub live_policy_version: Option<i32>,
    /// The highest `PolicyVersionId` ever minted here. A version is immutable, so its id
    /// is never reused: the next put takes this plus one even when the version that held
    /// it has since been deleted.
    pub policy_version_counter: i32,
    /// The instant the `UPDATING` -> `AVAILABLE` transition the clock owes becomes due,
    /// set by the update that reconfigured this core network — a description the caller
    /// supplied, or the execution that put a policy version in force — and `None` while no
    /// update has ever touched it. See [`CoreNetworkRecord::state_at`].
    pub updating_until: Option<SystemTime>,
    /// Whether the CREATE of this core network carried a policy document, which is what
    /// makes its first version the one [`Self::created_policy_document`] answers.
    pub created_with_policy_document: bool,
    /// The `ClientToken` of the request that minted this record, kept so a retry of that
    /// request answers this record instead of minting a second one.
    pub client_token: Option<String>,
    pub tags: Vec<Tag>,
}

impl CoreNetworkRecord {
    /// The document the CREATE of this core network carried: its first policy version,
    /// `None` for a core network created without a document.
    ///
    /// The version is the FIRST one of a core network the create gave a document to
    /// ([`Self::created_with_policy_document`]), so a put that mints version 1 on a core
    /// network created without a document does not make this answer the put's document
    /// — not even once that version's change set has been executed. This, and not the LIVE
    /// document, is what a retried create compares itself against: the LIVE document moves
    /// to whichever version is executed next, while the document a create carried is the
    /// one it is idempotent in.
    pub fn created_policy_document(&self) -> Option<&str> {
        if !self.created_with_policy_document {
            return None;
        }
        self.policy_versions
            .get(&FIRST_POLICY_VERSION)
            .map(|version| version.document.as_str())
    }

    /// The highest `PolicyVersionId` this core network still holds, or `None` while it
    /// holds no version at all.
    pub fn latest_policy_version(&self) -> Option<i32> {
        self.policy_versions.keys().next_back().copied()
    }

    /// The alias of the version `version_id`: `LIVE` for the version in force, `LATEST`
    /// for the newest version held when that is not the one in force, and none for every
    /// other version — the model allows a policy a single alias
    /// ([`POLICY_ALIAS_LIVE`]), and the policy in force is the one a caller means by it.
    pub fn policy_alias_of(&self, version_id: i32) -> Option<&'static str> {
        if self.live_policy_version == Some(version_id) {
            Some(POLICY_ALIAS_LIVE)
        } else if self.latest_policy_version() == Some(version_id) {
            Some(POLICY_ALIAS_LATEST)
        } else {
            None
        }
    }

    /// The modelled `CoreNetworkPolicy` for the version `version_id` as of `now`, or
    /// `None` when this core network holds no such version.
    pub fn policy_shape(&self, version_id: i32, now: SystemTime) -> Option<CoreNetworkPolicy> {
        let alias = self.policy_alias_of(version_id);
        self.policy_versions
            .get(&version_id)
            .map(|version| version.to_shape(alias, now))
    }

    /// Every policy version this core network holds as of `now`, each keyed by
    /// [`policy_version_page_key`] so [`paginate`] can page over them.
    ///
    /// In the order of the HISTORY — the order of the key the versions are stored under,
    /// oldest version id first — which is the key order every paginated listing of this
    /// service reports in and the order its page tokens resume after.
    pub fn policy_version_entries(
        &self,
        now: SystemTime,
    ) -> Vec<(String, CoreNetworkPolicyVersion)> {
        self.policy_versions
            .values()
            .map(|version| {
                (
                    policy_version_page_key(version.version_id),
                    version.to_version_shape(self.policy_alias_of(version.version_id), now),
                )
            })
            .collect()
    }

    /// The topology of the policy in force on this core network: the one derived from the
    /// LIVE version's document where that document was accepted
    /// ([`CoreNetworkPolicyRecord::topology`]), or the topology of no policy at all
    /// ([`NO_POLICY_IN_FORCE`]) while no version is in force — a core network created
    /// without a policy document, until a version's change set is executed.
    ///
    /// Derived data is kept beside the document it comes from and nowhere else, so this is
    /// a view of the LIVE version rather than a second copy: putting another version in
    /// force moves which version answers here and copies nothing.
    pub fn topology(&self) -> &PolicyTopology {
        self.live_policy_version
            .and_then(|live| self.policy_versions.get(&live))
            .map_or(&NO_POLICY_IN_FORCE, |live| &live.topology)
    }

    /// Store `document`, whose derived topology is `topology`, as this core network's next
    /// policy version and answer that version's modelled `CoreNetworkPolicy`.
    ///
    /// `topology` is the answer [`policy_topology`] gives for `document`, derived by the
    /// caller where the document was ACCEPTED and before the store was locked, so
    /// appending a version parses no policy document: the work done here is a comparison
    /// of two topologies both already derived and both bounded by
    /// [`validate_policy_document`].
    ///
    /// `executed` says whether this version's change set is being executed, which is what
    /// puts its document in force: an executed version becomes the LIVE one. A freshly
    /// minted version always carries the highest id in the history, so its alias follows
    /// from `executed` alone — `LIVE` for the version just put in force, `LATEST`
    /// otherwise — and the answer is built from the record itself rather than looked up
    /// again.
    pub fn add_policy_version(
        &mut self,
        document: String,
        topology: PolicyTopology,
        description: Option<String>,
        client_token: Option<String>,
        now: SystemTime,
        executed: bool,
    ) -> CoreNetworkPolicy {
        let version_id = self.policy_version_counter.saturating_add(1);
        self.policy_version_counter = version_id;
        // The change set generated with the version: the difference between the policy in
        // force right now and the one being stored.
        let changes = policy_changes(self.topology(), &topology);
        let record = CoreNetworkPolicyRecord {
            core_network_id: self.id.clone(),
            version_id,
            description,
            created_at: now,
            settled_at: now + SETTLE_DELAY,
            document,
            topology,
            client_token,
            executed_at: executed.then_some(now),
            // Freshly generated against the document in force a moment ago, so it is the
            // comparison that holds right now; a later execution of ANOTHER version is
            // what supersedes it.
            out_of_date: false,
            changes,
        };
        let alias = if executed {
            POLICY_ALIAS_LIVE
        } else {
            POLICY_ALIAS_LATEST
        };
        let shape = record.to_shape(Some(alias), now);
        self.policy_versions.insert(version_id, record);
        if executed {
            self.live_policy_version = Some(version_id);
        }
        shape
    }

    /// Put the policy version `version_id`'s document in force: what executing that
    /// version's change set does. A no-op for a version this core network does not hold,
    /// which a caller refuses before reaching here.
    ///
    /// Executing a change set makes its version the LIVE one — and with it the topology
    /// its document describes the one the core network reports ([`Self::topology`]) —
    /// starts the `EXECUTING` ->
    /// `EXECUTION_SUCCEEDED` span the clock then owes that change set (and with it the
    /// `IN_PROGRESS` -> `COMPLETE` span of each of its changes,
    /// [`CoreNetworkPolicyRecord::change_events_at`]) and the `UPDATING` -> `AVAILABLE`
    /// span it owes the core network being deployed to. Every OTHER change set still
    /// waiting to be executed described its differences from the document this one
    /// replaces, so none of them is a comparison that still holds: they are superseded
    /// ([`CoreNetworkPolicyRecord::out_of_date`]).
    pub fn execute_policy_version(&mut self, version_id: i32, now: SystemTime) {
        let Some(version) = self.policy_versions.get_mut(&version_id) else {
            return;
        };
        version.executed_at = Some(now);
        version.settled_at = now + SETTLE_DELAY;
        for (id, superseded) in self.policy_versions.iter_mut() {
            if *id != version_id && superseded.executed_at.is_none() {
                superseded.out_of_date = true;
            }
        }
        // The topology this core network reports follows from WHICH version is live: it was
        // derived where that version's document was accepted, so putting the document in
        // force re-parses nothing here.
        self.live_policy_version = Some(version_id);
        self.updating_until = Some(now + SETTLE_DELAY);
    }

    /// This core network's lifecycle state as of `now`.
    ///
    /// The transition is a function of the clock, so reading one never writes: a core
    /// network is `UPDATING` while a reconfiguration of it is still outstanding — a policy
    /// version being put in force, or an update of its own members — and `AVAILABLE` once
    /// none is, which is the state it is created in. `CreateCoreNetwork` answers the core
    /// network ALREADY `AVAILABLE`, so nothing is owed on its creation and the modelled
    /// `CREATING` ([`STATE_CREATING`]) is a state no core network of this service passes
    /// through.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.updating_until.is_some_and(|due| now < due) {
            STATE_UPDATING
        } else {
            STATE_AVAILABLE
        }
    }

    /// The modelled `CoreNetwork` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> CoreNetwork {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `CoreNetwork` for this record, forcing `state` — used by a delete,
    /// which reports the record it just removed as `DELETING`.
    ///
    /// A core network's segments and edges are the ones the policy in force configures, so
    /// a record being REMOVED — the one state a stored core network never reports itself in
    /// — carries no topology, and every other state carries the one derived from the stored
    /// policy document. A policy being deployed over it (`UPDATING`) does not take the
    /// topology away: a policy IS in force there, the one that execution put in force.
    pub fn to_shape_with_state(&self, state: &str) -> CoreNetwork {
        let topology = if state == STATE_DELETING {
            &NO_POLICY_IN_FORCE
        } else {
            self.topology()
        };
        CoreNetwork {
            global_network_id: Some(self.global_network_id.clone()),
            core_network_id: Some(self.id.clone()),
            core_network_arn: Some(self.arn.clone()),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            segments: Some(topology.to_segments()),
            network_function_groups: Some(topology.to_network_function_groups()),
            edges: Some(topology.edges.clone()),
            tags: Some(self.tags.clone()),
        }
    }
}
