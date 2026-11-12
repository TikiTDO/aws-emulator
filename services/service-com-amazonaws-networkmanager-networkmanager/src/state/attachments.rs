// SPDX-License-Identifier: CC0-1.0

//! The stored peerings, attachments and Connect peers of a core network, and the
//! modelled shapes every read of one answers.

use super::*;

/// A stored peering: what joins a core network, at ONE of its edge locations, to a
/// resource of another service — a transit gateway, the one kind of peering the model
/// carries ([`PEERING_TYPE_TRANSIT_GATEWAY`]).
///
/// A peering is a resource of its own, with an id, an ARN and tags, and it is what a
/// transit gateway route table attachment is created OVER
/// ([`AttachmentDetail::TransitGatewayRouteTable`]): the attachment takes its core network
/// and its edge location from the peering rather than from its own request, so the two can
/// never disagree about which core network the route table is reaching.
///
/// Like every record here, `state` is not a field but a function of what is stored and the
/// clock ([`Self::state_at`]), so reporting a peering never writes.
#[derive(Debug, Clone)]
pub struct PeeringRecord {
    pub id: String,
    pub arn: String,
    pub core_network_id: String,
    /// The ARN of [`Self::core_network_id`], kept so a read never has to resolve the
    /// parent to report `Peering$CoreNetworkArn`.
    pub core_network_arn: String,
    /// One of `ops::types::PEERING_TYPE_VALUES`, which carries the single value
    /// [`PEERING_TYPE_TRANSIT_GATEWAY`].
    pub peering_type: &'static str,
    /// The edge location the peering sits at: one of the locations the core network's
    /// policy configures ([`PolicyTopology::has_edge_location`]), which is the Region the
    /// peered resource is in.
    pub edge_location: String,
    /// The ARN of the resource peered with — the transit gateway for a transit gateway
    /// peering, which is also the `TransitGatewayArn` that shape reports.
    pub resource_arn: String,
    /// The id of the peering attachment this peering leaves on the peered transit gateway
    /// ([`TRANSIT_GATEWAY_PEERING_ATTACHMENT_KIND`]): a resource of the peered gateway, so
    /// it is minted here rather than taken from any request member.
    pub transit_gateway_peering_attachment_id: String,
    pub created_at: SystemTime,
    /// The instant the `CREATING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The `ClientToken` of the request that minted this record, kept so a retry of that
    /// request answers this record instead of minting a second peering.
    pub client_token: Option<String>,
    pub tags: Vec<Tag>,
}

impl PeeringRecord {
    /// This peering's lifecycle state as of `now`.
    ///
    /// `PeeringState` carries CREATING, FAILED, AVAILABLE and DELETING (`smithy_query
    /// shape=PeeringState`, mirrored in `ops::types::PEERING_STATE_VALUES`), two of which
    /// are the ones the clock decides: a peering under way waits in [`STATE_CREATING`]
    /// until the transition the clock owes falls due and is [`STATE_AVAILABLE`] after it.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_CREATING
        }
    }

    /// The modelled `Peering` envelope of this record, as of `now`.
    pub fn to_peering(&self, now: SystemTime) -> Peering {
        self.to_peering_with_state(self.state_at(now))
    }

    /// The modelled `Peering` envelope of this record in `state`: what a DELETE reports for
    /// the record it has just removed, which is no longer stored and so has no state the
    /// clock could compute. The same envelope every other read of a peering answers, so a
    /// delete reports the peering it removed in full rather than a shell carrying only an id.
    pub fn to_peering_with_state(&self, state: &str) -> Peering {
        Peering {
            core_network_id: Some(self.core_network_id.clone()),
            core_network_arn: Some(self.core_network_arn.clone()),
            peering_id: Some(self.id.clone()),
            owner_account_id: Some(::engine::emulator_account_id().to_string()),
            peering_type: Some(self.peering_type.to_string()),
            state: Some(state.to_string()),
            edge_location: Some(self.edge_location.clone()),
            resource_arn: Some(self.resource_arn.clone()),
            tags: Some(self.tags.clone()),
            created_at: Some(epoch_seconds(self.created_at)),
            last_modification_errors: no_reported_errors(),
        }
    }

    /// The modelled `TransitGatewayPeering` for this record as of `now`: the envelope
    /// above plus the two members only a transit gateway peering carries (`smithy_query
    /// shape=TransitGatewayPeering`).
    ///
    /// `TransitGatewayArn` is [`Self::resource_arn`]: the resource a transit gateway
    /// peering peers with IS the transit gateway, so it is not stored twice.
    pub fn to_transit_gateway_peering(&self, now: SystemTime) -> TransitGatewayPeering {
        TransitGatewayPeering {
            peering: Some(self.to_peering(now)),
            transit_gateway_arn: Some(self.resource_arn.clone()),
            transit_gateway_peering_attachment_id: Some(
                self.transit_gateway_peering_attachment_id.clone(),
            ),
        }
    }
}

/// A stored attachment: what joins one resource to ONE edge location of a core network.
///
/// Every attachment kind shares the modelled `Attachment` envelope (`smithy_query
/// shape=Attachment`) — the core network it belongs to, its type, its state, its edge
/// location, the ARN of the resource it attaches and its tags — and each kind adds its own
/// members on top, which is what [`AttachmentDetail`] holds. One record and one map
/// therefore serve every kind: `GetConnectAttachment`, `GetVpcAttachment` and
/// `GetSiteToSiteVpnAttachment` read the same store the quota, the inventory count and the
/// core-network delete refusal are computed from, so no two operations can disagree about
/// which attachments exist.
///
/// Like the records above, `state` is not a field but a function of what is stored and the
/// clock ([`Self::state_at`]), so reporting an attachment never writes. The one thing the
/// clock does NOT decide is the acceptance an `attachment-policies` rule can ask for: an
/// attachment waiting for one stays in [`STATE_PENDING_ATTACHMENT_ACCEPTANCE`] until
/// `AcceptAttachment` grants it ([`Self::accept_at`]) or `RejectAttachment` turns it down
/// ([`Self::reject_at`]), which leaves it [`STATE_REJECTED`] for good.
#[derive(Debug, Clone)]
pub struct AttachmentRecord {
    pub id: String,
    pub arn: String,
    pub core_network_id: String,
    /// The ARN of [`Self::core_network_id`], kept so a read never has to resolve the
    /// parent to report `Attachment$CoreNetworkArn`.
    pub core_network_arn: String,
    /// One of `ops::types::ATTACHMENT_TYPE_VALUES` — [`ATTACHMENT_TYPE_VPC`],
    /// [`ATTACHMENT_TYPE_CONNECT`] or [`ATTACHMENT_TYPE_SITE_TO_SITE_VPN`] for the three
    /// kinds this slice mints.
    pub attachment_type: &'static str,
    /// The edge location the attachment sits at: one of the locations the core network's
    /// policy configures ([`PolicyTopology::has_edge_location`]). For a kind that spans
    /// SEVERAL of them this is the primary of those locations and is not reported on its
    /// own (see [`AttachmentDetail::DirectConnectGateway`]).
    pub edge_location: String,
    /// The ARN of the resource this attachment attaches: the VPC for a VPC attachment, the
    /// VPN connection for a site-to-site VPN attachment, and for a Connect attachment the
    /// transport attachment it is carried over, which is the resource it is built on.
    pub resource_arn: String,
    /// The `RoutingPolicyLabel` the create carried, stored because the request supplied
    /// it. The modelled `Attachment` has no member for it (`smithy_query shape=Attachment`)
    /// — it is reported by `ListAttachmentRoutingPolicyAssociations`
    /// ([`Self::to_routing_policy_association`]) — so it is held here rather than dropped.
    pub routing_policy_label: Option<String>,
    /// The normalised idempotency token of the `PutAttachmentRoutingPolicyLabel` call that
    /// applied the label above, kept so a retry of that call answers it again instead of
    /// writing a second time and so the same token asking for another label is refused
    /// ([`replay_or_conflict_by`]). `None` for a label that came from the create, for one
    /// applied by a call carrying no token, and for an attachment carrying no label — none
    /// of which a token names.
    pub routing_policy_label_client_token: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `CREATING` -> `AVAILABLE` transition the clock owes becomes due, or
    /// `None` while the attachment is still waiting for the acceptance the core network
    /// policy asked for: nothing is being created until the request is accepted, so no
    /// transition is owed on it until then (see [`Self::accept_at`]).
    pub available_at: Option<SystemTime>,
    /// When the acceptance the policy asked for was granted — the instant the attachment
    /// last changed before the transition above falls due. `None` for an attachment no rule
    /// asked acceptance for, which never waited for one.
    pub accepted_at: Option<SystemTime>,
    /// When the attachment request was TURNED DOWN, or `None` for one nothing rejected.
    /// Terminal: an attachment holding an instant here is [`STATE_REJECTED`] from that
    /// instant on, is never created, and has no transition owed on it
    /// ([`Self::reject_at`]).
    pub rejected_at: Option<SystemTime>,
    /// The instant a CHANGE to this attachment was last requested — what
    /// `UpdateVpcAttachment` records — or `None` for one nothing has changed since it was
    /// created. What [`Self::state_at`] tells a creation in flight from a change in flight
    /// by, and what [`Self::updated_at`] reports until the transition falls due.
    pub changed_at: Option<SystemTime>,
    /// The `attachment-policies` rule of the core network policy in force that claimed this
    /// attachment, and what that rule put the attachment in: the
    /// `Attachment$AttachmentPolicyRuleNumber`, `Attachment$SegmentName` and
    /// `Attachment$NetworkFunctionGroupName` a read of it reports. All three are absent
    /// where no rule of that policy claims the attachment.
    pub attachment_policy_rule_number: Option<i32>,
    /// See [`Self::attachment_policy_rule_number`].
    pub segment_name: Option<String>,
    /// See [`Self::attachment_policy_rule_number`].
    pub network_function_group_name: Option<String>,
    /// The `ClientToken` of the request that minted this record, kept so a retry of that
    /// request answers this record instead of minting a second attachment.
    pub client_token: Option<String>,
    pub tags: Vec<Tag>,
    /// The members only this attachment's own kind has.
    pub detail: AttachmentDetail,
}

/// What one attachment kind adds to the shared [`AttachmentRecord`] envelope: the members
/// its own wrapper shape carries beside `Attachment` (`smithy_query
/// shape=VpcAttachment,ConnectAttachment,SiteToSiteVpnAttachment,TransitGatewayRouteTableAttachment,DirectConnectGatewayAttachment`),
/// plus the one envelope member only a single kind fills
/// ([`Self::DirectConnectGateway`]).
#[derive(Debug, Clone)]
pub enum AttachmentDetail {
    /// A VPC attachment: the subnets it reaches the VPC through, in the order the request
    /// listed them, and the VPC options it was created with.
    Vpc {
        subnet_arns: Vec<String>,
        options: Option<VpcOptions>,
    },
    /// A core network Connect attachment: the attachment carrying it and its tunnel
    /// options.
    Connect {
        transport_attachment_id: String,
        options: ConnectAttachmentOptions,
    },
    /// A site-to-site VPN attachment. It carries NO member of its own: beside
    /// `Attachment`, `SiteToSiteVpnAttachment` holds only `VpnConnectionArn`
    /// (`smithy_query shape=SiteToSiteVpnAttachment`), and that is the ARN of the resource
    /// this attachment attaches — [`AttachmentRecord::resource_arn`] — so storing it here
    /// would be a second copy of one value.
    SiteToSiteVpn,
    /// A Direct Connect gateway attachment: the edge locations it spans, in the order the
    /// request listed them.
    ///
    /// `DirectConnectGatewayArn` is [`AttachmentRecord::resource_arn`], as
    /// `VpnConnectionArn` is for the kind above, so it is not stored twice. What IS stored
    /// here is the one envelope member no other kind fills: this is the only kind that
    /// sits at SEVERAL edges of its core network, which the envelope reports as
    /// `Attachment$EdgeLocations` instead of `Attachment$EdgeLocation` ("The edge
    /// locations that the Direct Connect gateway is associated with. This is returned only
    /// for Direct Connect gateway attachments. All other attachment types retrun
    /// EdgeLocation", `smithy_query query="doc" shape="Attachment"
    /// member="EdgeLocations"`). [`AttachmentRecord::edge_location`] carries the first of
    /// them — the attachment's PRIMARY edge, the one an `attachment-policies` `region`
    /// condition is evaluated against ([`primary_edge_location`]) — and is never reported
    /// for this kind.
    DirectConnectGateway { edge_locations: Vec<String> },
    /// A transit gateway route table attachment: the transit gateway PEERING it was
    /// created over ([`PeeringRecord`]).
    ///
    /// `TransitGatewayRouteTableArn` is [`AttachmentRecord::resource_arn`], as
    /// `VpnConnectionArn` is for the site-to-site VPN kind, so it is not stored twice.
    /// What IS stored here is the peering, because this is the one kind whose edge
    /// location and core network come from a peering rather than from the attached
    /// resource: the route table is reached THROUGH that peering.
    TransitGatewayRouteTable { peering_id: String },
}

/// The PRIMARY edge of an attachment spanning `edge_locations`: the first of them, or
/// `None` where the list is empty — which no stored attachment has, since an attachment
/// joins a resource to at least one edge of its core network.
///
/// One place decides it, so the edge a multi-edge attachment carries as
/// [`AttachmentRecord::edge_location`] cannot drift between the create and a later update
/// of its locations.
pub fn primary_edge_location(edge_locations: &[String]) -> Option<&str> {
    edge_locations.first().map(String::as_str)
}

impl AttachmentRecord {
    /// This attachment's lifecycle state as of `now`.
    ///
    /// `AttachmentState` is a wider enum than the resource states above (REJECTED,
    /// PENDING_ATTACHMENT_ACCEPTANCE, CREATING, FAILED, AVAILABLE, UPDATING,
    /// PENDING_NETWORK_UPDATE, PENDING_TAG_ACCEPTANCE, DELETING: `smithy_query
    /// shape=AttachmentState`, mirrored in `ops::types::ATTACHMENT_STATE_VALUES`), and it
    /// shares two of them with [`CoreNetworkRecord::state_at`]: an attachment under way
    /// waits in [`STATE_CREATING`] until the transition the clock owes falls due and is
    /// [`STATE_AVAILABLE`] after it. An attachment the policy in force asked acceptance for
    /// waits in [`STATE_PENDING_ATTACHMENT_ACCEPTANCE`] INSTEAD, and the clock never moves
    /// it: only [`Self::accept_at`] does, after which the span above starts. A CHANGE to an
    /// attachment that is already there owes the same transition from
    /// [`ATTACHMENT_STATE_UPDATING`], which is why the state before the due instant depends
    /// on whether anything has changed the attachment since it was created
    /// ([`Self::changed_at`]).
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if self.rejected_at.is_some() {
            return STATE_REJECTED;
        }
        match self.available_at {
            None => STATE_PENDING_ATTACHMENT_ACCEPTANCE,
            Some(available_at) if now >= available_at => STATE_AVAILABLE,
            Some(_) if self.changed_at.is_some() => ATTACHMENT_STATE_UPDATING,
            Some(_) => STATE_CREATING,
        }
    }

    /// Whether this attachment is still waiting for the acceptance its core network policy
    /// asked for — the one thing `AcceptAttachment` and `RejectAttachment` can act on. An
    /// attachment already rejected waits for nothing: that decision was taken.
    pub fn awaits_acceptance(&self) -> bool {
        self.available_at.is_none() && self.rejected_at.is_none()
    }

    /// Whether anything was ever CREATED for this attachment — whether it is the attachment
    /// it describes rather than a request for one.
    ///
    /// An attachment still waiting for the acceptance its core network policy asked for has
    /// nothing created yet ([`Self::available_at`]), and one whose request was turned down
    /// never will have ([`Self::rejected_at`]); every other stored attachment is either
    /// coming into existence on the transition the clock owes it or already there, so it is
    /// something another resource can be built on ([`check_attachment_is_created`]).
    pub fn is_created(&self) -> bool {
        self.available_at.is_some() && self.rejected_at.is_none()
    }

    /// Grant the acceptance this attachment is waiting for, at `now`: the attachment starts
    /// being created, so the `CREATING` -> `AVAILABLE` transition the clock owes becomes due
    /// a [`SETTLE_DELAY`] later. Answers `false`, writing nothing, when the attachment was
    /// not waiting for one.
    pub fn accept_at(&mut self, now: SystemTime) -> bool {
        if !self.awaits_acceptance() {
            return false;
        }
        self.accepted_at = Some(now);
        self.available_at = Some(now + SETTLE_DELAY);
        true
    }

    /// Turn down the attachment request this attachment is waiting for, at `now`: it is
    /// never created, so no transition is owed on it and it is [`STATE_REJECTED`] from here
    /// on. Answers `false`, writing nothing, when the attachment was not waiting for an
    /// acceptance — the mirror of [`Self::accept_at`], so the two decisions are taken on
    /// exactly the same attachments and neither can overtake the other.
    pub fn reject_at(&mut self, now: SystemTime) -> bool {
        if !self.awaits_acceptance() {
            return false;
        }
        self.rejected_at = Some(now);
        true
    }

    /// When this attachment last changed, as of `now`: the create, the acceptance that
    /// started it, the rejection that turned its request down, the change an update
    /// requested, or the transition to `AVAILABLE` once that has fallen due. DERIVED from
    /// the stored instants, so a read reports a current `UpdatedAt` without writing one.
    pub fn updated_at(&self, now: SystemTime) -> SystemTime {
        match self.available_at {
            Some(available_at) if now >= available_at => available_at,
            _ => self
                .rejected_at
                .or(self.changed_at)
                .or(self.accepted_at)
                .unwrap_or(self.created_at),
        }
    }

    /// Overwrite the edge locations a Direct Connect gateway attachment spans, at `now`,
    /// keeping the first of them as the attachment's primary edge
    /// ([`primary_edge_location`]).
    ///
    /// The attachment has CHANGED, so — exactly as an accepted `UpdateVpcAttachment`
    /// records it — it is [`ATTACHMENT_STATE_UPDATING`] until the transition the clock owes
    /// falls due a [`SETTLE_DELAY`] later and [`STATE_AVAILABLE`] after it. Answers
    /// `false`, writing NOTHING, for an empty list, for a record of a kind that sits at one
    /// edge rather than spanning several, and for a list the record already holds — a
    /// change that asks for what is stored is no change, so no transition is owed on it.
    pub fn set_edge_locations(&mut self, requested: Vec<String>, now: SystemTime) -> bool {
        let Some(primary) = primary_edge_location(&requested).map(str::to_string) else {
            return false;
        };
        match &mut self.detail {
            AttachmentDetail::DirectConnectGateway { edge_locations } => {
                if *edge_locations == requested {
                    return false;
                }
                *edge_locations = requested;
                self.edge_location = primary;
                self.changed_at = Some(now);
                self.available_at = Some(now + SETTLE_DELAY);
                true
            }
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::TransitGatewayRouteTable { .. } => false,
        }
    }

    /// The ONE edge location this attachment sits at, or `None` for a kind that spans
    /// several of them instead.
    ///
    /// ONE place decides it, so every answer that names the single Region an attachment
    /// lives in — the `Attachment$EdgeLocation` of its own reader
    /// ([`Self::to_attachment_at`]) and the `NetworkResource$AwsRegion` of the global
    /// network's inventory ([`ComAmazonawsNetworkmanagerNetworkmanagerState::network_resources_of`])
    /// — reports the same one. A Direct Connect gateway attachment is the kind that spans
    /// several edges and reports them as `Attachment$EdgeLocations` instead ("This is
    /// returned only for Direct Connect gateway attachments. All other attachment types
    /// retrun EdgeLocation", `smithy_query query="doc" shape="Attachment"
    /// member="EdgeLocations"`), so it has no single edge to name.
    pub fn sole_edge_location(&self) -> Option<&str> {
        match &self.detail {
            AttachmentDetail::DirectConnectGateway { .. } => None,
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::TransitGatewayRouteTable { .. } => {
                Some(self.edge_location.as_str())
            }
        }
    }

    /// Whether this attachment sits at the edge location `location`.
    ///
    /// Every kind but one sits at ONE edge ([`Self::sole_edge_location`]), so that single
    /// edge decides it; a Direct Connect gateway attachment spans SEVERAL and sits at every
    /// one of them, so any of them decides it. That is the same split
    /// [`Self::to_attachment_at`] reports as `EdgeLocation` / `EdgeLocations`, so a listing
    /// selected on a location answers the attachments whose reported edges hold it and no
    /// other.
    pub fn sits_at(&self, location: &str) -> bool {
        match &self.detail {
            AttachmentDetail::DirectConnectGateway { edge_locations } => {
                edge_locations.iter().any(|edge| edge == location)
            }
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::TransitGatewayRouteTable { .. } => {
                self.sole_edge_location() == Some(location)
            }
        }
    }

    /// The telemetry resource type of the connection this attachment attaches — one of
    /// [`TELEMETRY_RESOURCE_TYPES`] — or `None` when what it attaches is not a connection
    /// whose session this service measures.
    ///
    /// A site-to-site VPN attachment attaches a VPN CONNECTION ([`VPN_CONNECTION_TYPE`],
    /// reported with the IPsec session health [`CONNECTION_TYPE_IPSEC`]), so tying one to a
    /// core network is what makes that connection's session known to the global network
    /// owning the core network ([`ComAmazonawsNetworkmanagerNetworkmanagerState::register_attachment_telemetry`]).
    /// What the other kinds attach is a VPC, a Direct Connect gateway, a transit gateway
    /// route table or another attachment: resources of the inventory rather than sessions,
    /// none of them a type `GetNetworkTelemetry` reports on. A Connect attachment carries
    /// BGP sessions, but each is a CONNECT PEER of its own, registered by the associate that
    /// ties that peer to the global network rather than by the attachment carrying it.
    pub fn telemetry_resource_type(&self) -> Option<&'static str> {
        match &self.detail {
            AttachmentDetail::SiteToSiteVpn => Some(VPN_CONNECTION_TYPE),
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::DirectConnectGateway { .. }
            | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
        }
    }

    /// The modelled `Attachment` envelope of this record, as of `now`.
    ///
    /// `EdgeLocations` is absent because "this is returned only for Direct Connect gateway
    /// attachments. All other attachment types retrun EdgeLocation" (`smithy_query
    /// query="doc" shape="Attachment" member="EdgeLocations"`).
    pub fn to_attachment(&self, now: SystemTime) -> Attachment {
        self.to_attachment_at(self.state_at(now), self.updated_at(now))
    }

    /// The modelled `Attachment` envelope of the record a delete at `now` just removed:
    /// [`STATE_DELETING`], which the clock no longer decides for it, and `now` as the
    /// instant it last changed — the delete IS that change.
    ///
    /// The sibling `to_shape_with_state` of the records above forces only the state because
    /// their shapes carry no `UpdatedAt` member at all (`GlobalNetwork`, `Site`, `Link`,
    /// `Device`, `Connection`, `CoreNetwork` each carry `CreatedAt` only); `Attachment` is
    /// the one shape of this service that reports when the resource last changed
    /// (`smithy_query shape=Attachment`), so it is the one delete that has an instant to
    /// report.
    pub fn to_deleted_attachment(&self, now: SystemTime) -> Attachment {
        self.to_attachment_at(STATE_DELETING, now)
    }

    /// The shared body of the two above: the envelope with `state` and `updated_at`
    /// supplied, so neither spelling of a read can disagree with the other about the rest.
    pub(super) fn to_attachment_at(&self, state: &str, updated_at: SystemTime) -> Attachment {
        Attachment {
            core_network_id: Some(self.core_network_id.clone()),
            core_network_arn: Some(self.core_network_arn.clone()),
            attachment_id: Some(self.id.clone()),
            owner_account_id: Some(::engine::emulator_account_id().to_string()),
            attachment_type: Some(self.attachment_type.to_string()),
            state: Some(state.to_string()),
            // Exactly ONE of the two edge members is reported, and which one is a function
            // of the kind ([`Self::sole_edge_location`]). Neither is a copy of the other:
            // the absent member stays absent.
            edge_location: self.sole_edge_location().map(str::to_string),
            edge_locations: match &self.detail {
                AttachmentDetail::DirectConnectGateway { edge_locations } => {
                    Some(edge_locations.clone())
                }
                AttachmentDetail::Vpc { .. }
                | AttachmentDetail::Connect { .. }
                | AttachmentDetail::SiteToSiteVpn
                | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
            },
            resource_arn: Some(self.resource_arn.clone()),
            // Which `attachment-policies` rule of the core network policy claimed this
            // attachment, and the segment or network function group that rule put it in,
            // were decided where the attachment was created, from the policy in force on
            // its core network, and are reported from the record rather than re-derived.
            attachment_policy_rule_number: self.attachment_policy_rule_number,
            segment_name: self.segment_name.clone(),
            network_function_group_name: self.network_function_group_name.clone(),
            tags: Some(self.tags.clone()),
            // A proposed change exists only while a tag or segment change is awaiting
            // acceptance, which nothing here can start.
            proposed_segment_change: None,
            proposed_network_function_group_change: None,
            created_at: Some(epoch_seconds(self.created_at)),
            updated_at: Some(epoch_seconds(updated_at)),
            last_modification_errors: no_reported_errors(),
        }
    }

    /// The modelled `ConnectAttachment` for this record as of `now`, or `None` when the
    /// record is not a Connect attachment — which is how `GetConnectAttachment` answers
    /// the id of an attachment of another kind as not found.
    pub fn to_connect_attachment(&self, now: SystemTime) -> Option<ConnectAttachment> {
        match &self.detail {
            AttachmentDetail::Connect {
                transport_attachment_id,
                options,
            } => Some(ConnectAttachment {
                attachment: Some(self.to_attachment(now)),
                transport_attachment_id: Some(transport_attachment_id.clone()),
                options: Some(options.clone()),
            }),
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::DirectConnectGateway { .. }
            | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
        }
    }

    /// The modelled `VpcAttachment` for this record as of `now`, or `None` when the record
    /// is not a VPC attachment. See [`Self::to_connect_attachment`].
    pub fn to_vpc_attachment(&self, now: SystemTime) -> Option<VpcAttachment> {
        match &self.detail {
            AttachmentDetail::Vpc {
                subnet_arns,
                options,
            } => Some(VpcAttachment {
                attachment: Some(self.to_attachment(now)),
                subnet_arns: Some(subnet_arns.clone()),
                options: options.clone(),
            }),
            AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::DirectConnectGateway { .. }
            | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
        }
    }

    /// The modelled `SiteToSiteVpnAttachment` for this record as of `now`, or `None` when
    /// the record is not a site-to-site VPN attachment. See
    /// [`Self::to_connect_attachment`].
    ///
    /// `VpnConnectionArn` is [`Self::resource_arn`]: the resource a site-to-site VPN
    /// attachment attaches IS the VPN connection.
    pub fn to_site_to_site_vpn_attachment(
        &self,
        now: SystemTime,
    ) -> Option<SiteToSiteVpnAttachment> {
        match &self.detail {
            AttachmentDetail::SiteToSiteVpn => Some(SiteToSiteVpnAttachment {
                attachment: Some(self.to_attachment(now)),
                vpn_connection_arn: Some(self.resource_arn.clone()),
            }),
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::DirectConnectGateway { .. }
            | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
        }
    }

    /// The modelled `DirectConnectGatewayAttachment` for this record as of `now`, or `None`
    /// when the record is not a Direct Connect gateway attachment. See
    /// [`Self::to_connect_attachment`].
    ///
    /// `DirectConnectGatewayArn` is [`Self::resource_arn`]: the resource this attachment
    /// attaches IS the Direct Connect gateway.
    pub fn to_direct_connect_gateway_attachment(
        &self,
        now: SystemTime,
    ) -> Option<DirectConnectGatewayAttachment> {
        match &self.detail {
            AttachmentDetail::DirectConnectGateway { .. } => Some(DirectConnectGatewayAttachment {
                attachment: Some(self.to_attachment(now)),
                direct_connect_gateway_arn: Some(self.resource_arn.clone()),
            }),
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::TransitGatewayRouteTable { .. } => None,
        }
    }

    /// The modelled `TransitGatewayRouteTableAttachment` for this record as of `now`, or
    /// `None` when the record is not a transit gateway route table attachment. See
    /// [`Self::to_connect_attachment`].
    ///
    /// `TransitGatewayRouteTableArn` is [`Self::resource_arn`]: the resource this
    /// attachment attaches IS the transit gateway route table.
    pub fn to_transit_gateway_route_table_attachment(
        &self,
        now: SystemTime,
    ) -> Option<TransitGatewayRouteTableAttachment> {
        match &self.detail {
            AttachmentDetail::TransitGatewayRouteTable { peering_id } => {
                Some(TransitGatewayRouteTableAttachment {
                    attachment: Some(self.to_attachment(now)),
                    peering_id: Some(peering_id.clone()),
                    transit_gateway_route_table_arn: Some(self.resource_arn.clone()),
                })
            }
            AttachmentDetail::Vpc { .. }
            | AttachmentDetail::Connect { .. }
            | AttachmentDetail::SiteToSiteVpn
            | AttachmentDetail::DirectConnectGateway { .. } => None,
        }
    }

    /// The modelled `AttachmentRoutingPolicyAssociationSummary` of this record, or `None`
    /// when the attachment carries no routing policy label: the label is what ties an
    /// attachment to the routing policies of its core network, so an attachment without one
    /// has no routing policy association to report.
    pub fn to_routing_policy_association(&self) -> Option<AttachmentRoutingPolicyAssociationSummary> {
        let label = self.routing_policy_label.clone()?;
        Some(AttachmentRoutingPolicyAssociationSummary {
            attachment_id: Some(self.id.clone()),
            routing_policy_label: Some(label),
            // The routing policies an attachment's label matches are declared by the core
            // network policy document, which this slice parses for its edges, segments,
            // network function groups and attachment policies ([`policy_topology`]) and for
            // nothing else: no operation of this service creates, names or changes a routing
            // policy, so there is no stored policy for a label to match. Both members are
            // therefore absent rather than carrying names nothing declared.
            pending_routing_policies: None,
            associated_routing_policies: None,
        })
    }
}

/// Check the attachment a request wants to build a resource ON — the transport a Connect
/// attachment is carried over, the Connect attachment a Connect peer runs inside — against
/// what it is: a resource is built on an attachment that EXISTS, and an attachment whose
/// request nothing has granted yet, or that was turned down, is not one
/// ([`AttachmentRecord::is_created`]).
///
/// The refusal is the modelled `ValidationException` (HTTP 400) naming the member that
/// carried the reference, exactly as the other refusals of that reference do: the request
/// names an attachment that cannot carry anything, which is a fault of that member's value
/// against stored state ([`VALIDATION_REASON_OTHER`]), and the state the attachment is in
/// is reported so the caller knows whether an acceptance is owed on it or the decision was
/// already taken against it.
///
/// ONE function owns the rule, so no two creates can disagree about which attachments can
/// be built on. `member` is the request member carrying the reference and `built` names the
/// resource the request asked for, as the message reads it.
pub fn check_attachment_is_created(
    member: &str,
    attachment: &AttachmentRecord,
    built: &str,
    now: SystemTime,
) -> Result<(), ValidationException> {
    if attachment.is_created() {
        return Ok(());
    }
    Err(field_rejection(
        member,
        VALIDATION_REASON_OTHER,
        format!(
            "attachment {} is {}; {built} is built on an attachment that exists",
            attachment.id,
            attachment.state_at(now)
        ),
    ))
}

/// A stored core network Connect peer: the BGP peering one CONNECT attachment carries to
/// an appliance outside AWS.
///
/// A peer belongs to ONE Connect attachment, and therefore to that attachment's core
/// network and to the single edge location the attachment sits at: those three are read
/// from the attachment where the peer is created and kept here, so a peer reports the edge
/// its tunnel really lands on rather than one a request named. The tunnel PROTOCOL is the
/// attachment's too — a Connect attachment "is a GRE-based tunnel attachment"
/// (`smithy_query query="doc" shape="CreateConnectAttachment"`) unless its options ask for
/// `NO_ENCAP` — which is why `CreateConnectPeer` takes no protocol of its own.
///
/// Like the records above, `state` is not a field but a function of what is stored and the
/// clock ([`Self::state_at`]), so reporting a peer never writes.
#[derive(Debug, Clone)]
pub struct ConnectPeerRecord {
    pub id: String,
    pub arn: String,
    /// The Connect attachment this peer is carried over.
    pub connect_attachment_id: String,
    /// The core network and edge location of that attachment. Kept here so a read never
    /// has to resolve the attachment to report `ConnectPeer$CoreNetworkId` and
    /// `ConnectPeer$EdgeLocation`.
    pub core_network_id: String,
    /// See [`Self::core_network_id`].
    pub edge_location: String,
    /// The tunnel protocol of that attachment — one of `ops::types::TUNNEL_PROTOCOL_VALUES`
    /// — or `None` where its options named none, which is what the peer's configuration
    /// reports as `ConnectPeerConfiguration$Protocol`.
    pub protocol: Option<String>,
    /// The addresses the tunnel's two ends carry: the core network side, which only a GRE
    /// peering needs ("This only applies only when the protocol is GRE", `smithy_query
    /// query="doc" shape="CreateConnectPeerRequest" member="CoreNetworkAddress"`), and the
    /// appliance side, which every peering requires.
    ///
    /// A peering whose [`Self::protocol`] is another one carries NONE: the create that
    /// named it against that protocol is refused rather than stored
    /// ([`check_protocol_scoped_member`]), so this is held only where it applies.
    pub core_network_address: Option<String>,
    /// See [`Self::core_network_address`].
    pub peer_address: String,
    /// The inside IP addresses the two ends run BGP over, as the request listed them,
    /// bounded by [`MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER`].
    pub inside_cidr_blocks: Vec<String>,
    /// The two ends' AS numbers: the core network's is the ASN its policy assigns the edge
    /// this peer lands on ([`PolicyTopology::edge_asn`]), and the appliance's is the one the
    /// request's `BgpOptions` named. Either is absent where the policy assigns the edge none
    /// or the request named none.
    pub core_network_asn: Option<i64>,
    /// See [`Self::core_network_asn`].
    pub peer_asn: Option<i64>,
    /// The subnet the peer reaches its appliance through, which only a `NO_ENCAP` peering
    /// needs (`smithy_query query="doc" shape="CreateConnectPeerRequest"
    /// member="SubnetArn"`), or `None` where the request named none — the modelled pattern
    /// admits the empty string, which names no subnet and is held as none.
    ///
    /// Like [`Self::core_network_address`], a peering whose [`Self::protocol`] is another
    /// one carries none ([`check_protocol_scoped_member`]).
    pub subnet_arn: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `CREATING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    /// The `ClientToken` of the request that minted this record, kept so a retry of that
    /// request answers this record instead of minting a second peer.
    pub client_token: Option<String>,
    pub tags: Vec<Tag>,
}

impl ConnectPeerRecord {
    /// This peer's lifecycle state as of `now`.
    ///
    /// `ConnectPeerState` enumerates CREATING, FAILED, AVAILABLE and DELETING
    /// (`smithy_query shape=ConnectPeerState`, mirrored in
    /// `ops::types::CONNECT_PEER_STATE_VALUES`), and shares the two the clock decides with
    /// [`CoreNetworkRecord::state_at`]: a peer being created waits in [`STATE_CREATING`]
    /// until the transition the clock owes falls due and is [`STATE_AVAILABLE`] after it.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_CREATING
        }
    }

    /// The modelled `ConnectPeerConfiguration` of this peer: the two tunnel addresses, the
    /// inside CIDR blocks, the protocol it inherits from its attachment, and the BGP
    /// session that configuration describes.
    ///
    /// ONE BGP configuration, because one peer joins one core network edge to one
    /// appliance: its two AS numbers and its two addresses are exactly the stored ends of
    /// this peering, so nothing here is invented.
    pub fn configuration(&self) -> ConnectPeerConfiguration {
        ConnectPeerConfiguration {
            core_network_address: self.core_network_address.clone(),
            peer_address: Some(self.peer_address.clone()),
            inside_cidr_blocks: Some(self.inside_cidr_blocks.clone()),
            protocol: self.protocol.clone(),
            bgp_configurations: Some(vec![ConnectPeerBgpConfiguration {
                core_network_asn: self.core_network_asn,
                peer_asn: self.peer_asn,
                core_network_address: self.core_network_address.clone(),
                peer_address: Some(self.peer_address.clone()),
            }]),
        }
    }

    /// The modelled `ConnectPeer` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> ConnectPeer {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `ConnectPeer` for this record in `state`: what a DELETE reports for the
    /// record it has just removed, which is no longer stored and so has no state the clock
    /// could compute. The same shape every other read of a peer answers, so a delete
    /// reports the configuration the peer was stored with rather than an empty shell.
    pub fn to_shape_with_state(&self, state: &str) -> ConnectPeer {
        ConnectPeer {
            core_network_id: Some(self.core_network_id.clone()),
            connect_attachment_id: Some(self.connect_attachment_id.clone()),
            connect_peer_id: Some(self.id.clone()),
            edge_location: Some(self.edge_location.clone()),
            state: Some(state.to_string()),
            created_at: Some(epoch_seconds(self.created_at)),
            configuration: Some(self.configuration()),
            tags: Some(self.tags.clone()),
            subnet_arn: self.subnet_arn.clone(),
            last_modification_errors: no_reported_errors(),
        }
    }

    /// The modelled `ConnectPeerSummary` for this record, as of `now`: the same stored
    /// record `ListConnectPeers` reports, without the configuration only a read of the one
    /// peer carries (`smithy_query shape=ConnectPeerSummary,ConnectPeer`).
    pub fn to_summary(&self, now: SystemTime) -> ConnectPeerSummary {
        ConnectPeerSummary {
            core_network_id: Some(self.core_network_id.clone()),
            connect_attachment_id: Some(self.connect_attachment_id.clone()),
            connect_peer_id: Some(self.id.clone()),
            edge_location: Some(self.edge_location.clone()),
            connect_peer_state: Some(self.state_at(now).to_string()),
            created_at: Some(epoch_seconds(self.created_at)),
            tags: Some(self.tags.clone()),
            subnet_arn: self.subnet_arn.clone(),
        }
    }
}
