// SPDX-License-Identifier: CC0-1.0

//! The store itself: the operations every handler of this service reaches the shared
//! state through.

use super::*;

impl ComAmazonawsNetworkmanagerNetworkmanagerState {
    /// Mint the next id for `prefix`, e.g. `global-network-00000000000000001`.
    ///
    /// Network Manager ids are `<kind>-<hex>` (the model's own patterns for the kinds
    /// that constrain them read `^core-network-([0-9a-f]{8,17})$`), so the suffix is 17
    /// lowercase hex digits of the monotonic counter.
    pub fn next_id(&mut self, prefix: &str) -> String {
        self.id_counter = self.id_counter.saturating_add(1);
        format!("{prefix}-{:017x}", self.id_counter)
    }

    /// Apply an organization service-access update to the stored record and answer the
    /// record it leaves behind.
    ///
    /// `enable` is the `ENABLE` / `DISABLE` the request's `Action` named (see
    /// [`SERVICE_ACCESS_ACTIONS`]).
    ///
    /// * `ENABLE` on an organization whose access is already `ENABLED` changes NOTHING —
    ///   not the organization id, and not the SLR deployment already in flight — so a
    ///   client that repeats the call does not restart the deployment it is waiting for.
    /// * `ENABLE` otherwise records `ENABLED` and starts a fresh SLR deployment, due
    ///   [`SETTLE_DELAY`] from `now`, covering the accounts of the organization.
    /// * `DISABLE` records `DISABLED` and drops the deployment: with no SLR deployed there
    ///   is no deployment status to report. The organization id is kept, so re-enabling
    ///   reports the same organization.
    pub fn update_organization_service_access(
        &mut self,
        enable: bool,
        now: SystemTime,
    ) -> OrganizationServiceAccessRecord {
        let held = self.organization_service_access.clone();
        let organization_id = match &held {
            Some(record) => record.organization_id.clone(),
            None => self.next_id(ORGANIZATION_KIND),
        };
        let record = match held {
            Some(record)
                if enable && record.service_access_status == SERVICE_ACCESS_ENABLED =>
            {
                record
            }
            _ if enable => OrganizationServiceAccessRecord {
                organization_id,
                service_access_status: SERVICE_ACCESS_ENABLED.to_string(),
                deployed_accounts: vec![::engine::emulator_account_id().to_string()],
                deployed_at: now + SETTLE_DELAY,
            },
            _ => OrganizationServiceAccessRecord {
                organization_id,
                service_access_status: SERVICE_ACCESS_DISABLED.to_string(),
                deployed_accounts: Vec::new(),
                deployed_at: now,
            },
        };
        self.organization_service_access = Some(record.clone());
        record
    }

    /// The ARN of a resource of this service that stands on its own — one whose identity
    /// needs no parent to be unique: `arn:aws:networkmanager::<account>:<kind>/<id>`.
    ///
    /// That is the form for the kinds this service identifies account-wide: a global
    /// network ([`GLOBAL_NETWORK_KIND`]), a core network ([`CORE_NETWORK_KIND`]) and an
    /// attachment ([`ATTACHMENT_KIND`]). A resource that exists only INSIDE a global
    /// network is named by [`Self::scoped_resource_arn`] instead.
    pub fn resource_arn(kind: &str, id: &str) -> String {
        ::engine::emulator_arn(ARN_NAMESPACE, ARN_REGION, &format!("{kind}/{id}"))
    }

    /// The ARN of a resource of this service that exists only inside a SCOPE — for every
    /// such kind here, the global network that owns it:
    /// `arn:aws:networkmanager::<account>:<kind>/<scope-id>/<id>`.
    ///
    /// The owner's id is part of the name because the resource has no identity outside it:
    /// a site ([`SITE_KIND`]), a link ([`LINK_KIND`]), a device ([`DEVICE_KIND`]) and a
    /// connection ([`CONNECTION_KIND`]) are each addressed through the global network that
    /// holds them, so their ARN names that global network ahead of their own id — which
    /// stays the LAST path segment, and is what [`parse_resource_arn`] reads back as the id
    /// a lookup keys on.
    pub fn scoped_resource_arn(kind: &str, scope_id: &str, id: &str) -> String {
        ::engine::emulator_arn(
            ARN_NAMESPACE,
            ARN_REGION,
            &format!("{kind}/{scope_id}/{id}"),
        )
    }

    /// How many sites `global_network_id` owns, for the per-global-network quota.
    pub fn site_count_of(&self, global_network_id: &str) -> usize {
        self.sites
            .values()
            .filter(|site| site.global_network_id == global_network_id)
            .count()
    }

    /// The ids of the sites owned by `global_network_id`, in `SiteId` order.
    pub fn site_ids_of(&self, global_network_id: &str) -> Vec<String> {
        self.sites
            .values()
            .filter(|site| site.global_network_id == global_network_id)
            .map(|site| site.id.clone())
            .collect()
    }

    /// How many links the global network `global_network_id` holds, across all its sites.
    pub fn link_count_of_network(&self, global_network_id: &str) -> usize {
        self.links
            .values()
            .filter(|link| link.global_network_id == global_network_id)
            .count()
    }

    /// How many devices the global network `global_network_id` holds, for the
    /// per-global-network quota and the network-resource inventory.
    pub fn device_count_of(&self, global_network_id: &str) -> usize {
        self.devices
            .values()
            .filter(|device| device.global_network_id == global_network_id)
            .count()
    }

    /// How many connections the global network `global_network_id` holds, for the
    /// per-global-network quota and the network-resource inventory.
    pub fn connection_count_of(&self, global_network_id: &str) -> usize {
        self.connections
            .values()
            .filter(|connection| connection.global_network_id == global_network_id)
            .count()
    }

    /// Every stored record of `global_network_id` that still NAMES the link `link_id`,
    /// paired with its kind in the order a refusal should list them — what
    /// [`dependents_block_delete`] turns into the dependency `ValidationException` of
    /// `DeleteLink`.
    ///
    /// A delete may not leave a stored record pointing at a resource no read can resolve
    /// any more, so what blocks it is EVERY reference to the link this store holds, whether
    /// the referring record requires one ([`LinkAssociationRecord`]) or only carries the
    /// link the request supplied ([`CustomerGatewayAssociationRecord`],
    /// [`ConnectionRecord`], [`TransitGatewayConnectPeerAssociationRecord`],
    /// [`ConnectPeerAssociationRecord`]) — a connection holds two such members, either of
    /// which names a link of the same global network. The two kinds the operation's own
    /// documentation names ("You must first disassociate the link from any devices and
    /// customer gateways", `smithy_query query="doc" shape="DeleteLink"`) come first.
    pub fn link_dependents(
        &self,
        global_network_id: &str,
        link_id: &str,
    ) -> Vec<(&'static str, usize)> {
        let names_link = |candidate: &Option<String>| candidate.as_deref() == Some(link_id);
        vec![
            (
                LINK_ASSOCIATION_KIND,
                self.link_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && association.link_id == link_id
                    })
                    .count(),
            ),
            (
                CUSTOMER_GATEWAY_ASSOCIATION_KIND,
                self.customer_gateway_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && names_link(&association.link_id)
                    })
                    .count(),
            ),
            (
                CONNECTION_KIND,
                self.connections
                    .values()
                    .filter(|connection| {
                        connection.global_network_id == global_network_id
                            && (names_link(&connection.link_id)
                                || names_link(&connection.connected_link_id))
                    })
                    .count(),
            ),
            (
                TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION_KIND,
                self.transit_gateway_connect_peer_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && names_link(&association.link_id)
                    })
                    .count(),
            ),
            (
                CONNECT_PEER_ASSOCIATION_KIND,
                self.connect_peer_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && names_link(&association.link_id)
                    })
                    .count(),
            ),
        ]
    }

    /// Every stored record of `global_network_id` that still NAMES the device `device_id`,
    /// paired with its kind in the order a refusal should list them — the
    /// [`link_dependents`](Self::link_dependents) of a device, for `DeleteDevice`.
    ///
    /// The same rule holds here: the four ties that each run over one device of the global
    /// network, and the connections whose two device members name it — "Creates a
    /// connection between two devices" (`smithy_query query="doc"
    /// shape="CreateConnection"`), which is a reference needing no association at all. The
    /// kinds the operation's own documentation names ("You must first disassociate the
    /// device from any links and customer gateways", `smithy_query query="doc"
    /// shape="DeleteDevice"`) come first.
    pub fn device_dependents(
        &self,
        global_network_id: &str,
        device_id: &str,
    ) -> Vec<(&'static str, usize)> {
        vec![
            (
                LINK_ASSOCIATION_KIND,
                self.link_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && association.device_id == device_id
                    })
                    .count(),
            ),
            (
                CUSTOMER_GATEWAY_ASSOCIATION_KIND,
                self.customer_gateway_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && association.device_id == device_id
                    })
                    .count(),
            ),
            (
                CONNECTION_KIND,
                self.connections
                    .values()
                    .filter(|connection| {
                        connection.global_network_id == global_network_id
                            && (connection.device_id == device_id
                                || connection.connected_device_id == device_id)
                    })
                    .count(),
            ),
            (
                TRANSIT_GATEWAY_CONNECT_PEER_ASSOCIATION_KIND,
                self.transit_gateway_connect_peer_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && association.device_id == device_id
                    })
                    .count(),
            ),
            (
                CONNECT_PEER_ASSOCIATION_KIND,
                self.connect_peer_associations
                    .values()
                    .filter(|association| {
                        association.global_network_id == global_network_id
                            && association.device_id == device_id
                    })
                    .count(),
            ),
        ]
    }

    /// The ids of the devices assigned to `site_id`, in `DeviceId` order.
    pub fn device_ids_of_site(&self, site_id: &str) -> Vec<String> {
        self.devices
            .values()
            .filter(|device| device.site_id.as_deref() == Some(site_id))
            .map(|device| device.id.clone())
            .collect()
    }

    /// How many core networks `global_network_id` owns, for the per-global-network quota.
    pub fn core_network_count_of(&self, global_network_id: &str) -> usize {
        self.core_networks
            .values()
            .filter(|core_network| core_network.global_network_id == global_network_id)
            .count()
    }

    /// The ids of the core networks owned by `global_network_id`, in `CoreNetworkId` order.
    pub fn core_network_ids_of(&self, global_network_id: &str) -> Vec<String> {
        self.core_networks
            .values()
            .filter(|core_network| core_network.global_network_id == global_network_id)
            .map(|core_network| core_network.id.clone())
            .collect()
    }

    /// How many attachments `core_network_id` holds, across every attachment kind, for the
    /// per-core-network quota and the core-network delete refusal.
    pub fn attachment_count_of_core_network(&self, core_network_id: &str) -> usize {
        self.attachments
            .values()
            .filter(|attachment| attachment.core_network_id == core_network_id)
            .count()
    }

    /// The attachment `attachment_id` names WITHIN the core network `core_network_id`, for
    /// the operations that address a stored attachment THROUGH its core network and mint
    /// nothing — the routing policy label ones.
    ///
    /// Both ids address resources that have to be stored already, so an id naming nothing
    /// stored is the service's not-found refusal rather than the field failure a create
    /// gives for a reference it would hang a new resource off. The resource the request
    /// reaches the attachment THROUGH is checked first, the rule every operation of this
    /// service that addresses a child of a core network follows. The attachment is then
    /// resolved within that core network: one id space holds every attachment kind and a
    /// label applies to any of them, so the kind is never checked — but an attachment
    /// stored under a DIFFERENT core network is not an attachment of the one addressed, so
    /// it is as absent here as an id nothing was ever stored for.
    pub fn attachment_of_core_network_mut(
        &mut self,
        core_network_id: &str,
        attachment_id: &str,
    ) -> Result<&mut AttachmentRecord, ResourceNotFoundException> {
        self.require_attachment_of_core_network(core_network_id, attachment_id)?;
        self.attachments
            .get_mut(attachment_id)
            .ok_or_else(|| resource_not_found(ATTACHMENT_KIND, attachment_id))
    }

    /// The same resolution as [`Self::attachment_of_core_network_mut`] without handing the
    /// record back: `Err` names whichever of the two resources the request addresses the
    /// store does not hold.
    ///
    /// For the point in such a call where the refusal is already owed but the record is not
    /// needed yet: a tokened write resolves the resources it addresses before it resolves
    /// its idempotency token, and that resolution walks the whole attachment map, across
    /// which no borrow of one record can be held.
    pub fn require_attachment_of_core_network(
        &self,
        core_network_id: &str,
        attachment_id: &str,
    ) -> Result<(), ResourceNotFoundException> {
        if !self.core_networks.contains_key(core_network_id) {
            return Err(resource_not_found(CORE_NETWORK_KIND, core_network_id));
        }
        if !self
            .attachments
            .get(attachment_id)
            .is_some_and(|record| record.core_network_id == core_network_id)
        {
            return Err(resource_not_found(ATTACHMENT_KIND, attachment_id));
        }
        Ok(())
    }

    /// How many peerings `core_network_id` holds, for the per-core-network quota and the
    /// core-network delete refusal: a peering hangs off exactly one core network, so
    /// deleting that core network while it holds one would orphan the peering.
    pub fn peering_count_of_core_network(&self, core_network_id: &str) -> usize {
        self.peerings
            .values()
            .filter(|peering| peering.core_network_id == core_network_id)
            .count()
    }

    /// How many customer gateways `global_network_id` is associated with, for the
    /// per-global-network quota.
    pub fn customer_gateway_association_count_of(&self, global_network_id: &str) -> usize {
        self.customer_gateway_associations
            .values()
            .filter(|association| association.global_network_id == global_network_id)
            .count()
    }

    /// How many transit gateways `global_network_id` holds a registration for, for the
    /// per-global-network quota.
    pub fn transit_gateway_registration_count_of(&self, global_network_id: &str) -> usize {
        self.transit_gateway_registrations
            .values()
            .filter(|registration| registration.global_network_id == global_network_id)
            .count()
    }

    /// How many transit gateway Connect peers `global_network_id` is associated with, for
    /// the per-global-network quota.
    pub fn transit_gateway_connect_peer_association_count_of(
        &self,
        global_network_id: &str,
    ) -> usize {
        self.transit_gateway_connect_peer_associations
            .values()
            .filter(|association| association.global_network_id == global_network_id)
            .count()
    }

    /// Whether `global_network_id` holds a transit gateway registration for a gateway that
    /// could carry a Connect peer living in `region` and owned by `account`.
    ///
    /// "You can only associate transit gateway Connect peers that have been created on a
    /// transit gateway that's registered in your global network." (`smithy_query
    /// query="doc" shape="AssociateTransitGatewayConnectPeer"`). WHICH gateway a given
    /// Connect peer was created on is Amazon VPC's record, which this service holds no
    /// inventory of, and the peer's own ARN does not name it; what the ARN does fix is the
    /// Region and the owning account, and a Connect peer always lives in the Region and
    /// account of the transit gateway it was created on. So the registrations that could
    /// carry the peer are those of a gateway in that same Region and account, and a global
    /// network holding none of them holds no gateway the peer can have been created on.
    pub fn carries_transit_gateway_in(
        &self,
        global_network_id: &str,
        region: &str,
        account: Option<&str>,
    ) -> bool {
        self.carries_transit_gateway_in_besides(global_network_id, region, account, None)
    }

    /// [`Self::carries_transit_gateway_in`] with the registration held under `besides`
    /// left out: what the global network would still carry once that registration is gone.
    pub(super) fn carries_transit_gateway_in_besides(
        &self,
        global_network_id: &str,
        region: &str,
        account: Option<&str>,
        besides: Option<&str>,
    ) -> bool {
        self.transit_gateway_registrations
            .iter()
            .filter(|(key, registration)| {
                registration.global_network_id == global_network_id
                    && Some(key.as_str()) != besides
            })
            .any(|(_, registration)| {
                ec2_resource_region(&registration.transit_gateway_arn, TRANSIT_GATEWAY_TYPE)
                    == Some(region)
                    && arn_account_id(&registration.transit_gateway_arn) == account
            })
    }

    /// How many stored transit gateway Connect peer associations of `global_network_id`
    /// would be left standing on no registration at all once the registration held under
    /// `registration_key` is gone — the count the deregister's dependency refusal is built
    /// from ([`dependents_block_delete`]).
    ///
    /// An association of this kind can only be made while the global network carries a
    /// registration of a gateway the peer could have been created on
    /// ([`Self::carries_transit_gateway_in`]), and it keeps standing on that precondition
    /// for as long as it is stored, so removing the LAST registration that satisfies it
    /// would leave the tie in exactly the state the associate refuses to create. Another
    /// registration in the peer's Region and account still satisfies it, which is why the
    /// question is asked of the registrations that would remain rather than of the one
    /// being removed.
    pub fn transit_gateway_connect_peer_associations_standing_on(
        &self,
        global_network_id: &str,
        registration_key: &str,
    ) -> usize {
        self.transit_gateway_connect_peer_associations
            .values()
            .filter(|association| association.global_network_id == global_network_id)
            .filter(|association| {
                ec2_resource_region(
                    &association.transit_gateway_connect_peer_arn,
                    TRANSIT_GATEWAY_CONNECT_PEER_TYPE,
                )
                .is_some_and(|region| {
                    !self.carries_transit_gateway_in_besides(
                        global_network_id,
                        region,
                        arn_account_id(&association.transit_gateway_connect_peer_arn),
                        Some(registration_key),
                    )
                })
            })
            .count()
    }

    /// Whether `global_network_id` carries, in `region`, something a VPN connection landing
    /// there can be attached to: a registered transit gateway in that Region, or a core
    /// network of this global network whose policy IN FORCE configures an edge there.
    ///
    /// "You can only associate customer gateways that are connected to a VPN attachment on a
    /// transit gateway or core network registered in your global network." (`smithy_query
    /// query="doc" shape="AssociateCustomerGateway"`). WHICH VPN connection a given customer
    /// gateway is connected to is Amazon VPC's record, which this service holds no inventory
    /// of, and the gateway's own ARN does not name it; what that ARN does fix is the Region
    /// the gateway lives in, and a VPN connection is a Regional resource, so the transit
    /// gateway it attaches to and the core network EDGE a site-to-site VPN attachment of it
    /// sits at ([`PolicyTopology::has_edge_location`], which is also what
    /// [`check_edge_location`] judges such an attachment against) are both in that same
    /// Region. So the carriers that could reach the gateway are a registration of a gateway
    /// in its Region and a core network edge configured there, and a global network holding
    /// neither holds nothing the gateway can be connected to a VPN attachment on.
    ///
    /// The owning ACCOUNT is not compared, unlike [`Self::carries_transit_gateway_in`]: a
    /// Connect peer is created ON the registered gateway and so belongs to the same account
    /// as it, while a VPN connection belongs to the account that owns the customer gateway
    /// and reaches a transit gateway or a core network edge the global network's own account
    /// owns (`RegisterTransitGateway` holds every registration to that account), which is a
    /// different account whenever the customer gateway is another account's.
    pub fn carries_vpn_attachment_in(&self, global_network_id: &str, region: &str) -> bool {
        let registered = self
            .transit_gateway_registrations
            .values()
            .filter(|registration| registration.global_network_id == global_network_id)
            .any(|registration| {
                ec2_resource_region(&registration.transit_gateway_arn, TRANSIT_GATEWAY_TYPE)
                    == Some(region)
            });
        registered
            || self
                .core_networks
                .values()
                .filter(|core_network| core_network.global_network_id == global_network_id)
                .any(|core_network| core_network.topology().has_edge_location(region))
    }

    /// How many managed prefix lists `core_network_id` is associated with, for the
    /// per-core-network quota.
    pub fn prefix_list_association_count_of(&self, core_network_id: &str) -> usize {
        self.prefix_list_associations
            .values()
            .filter(|association| association.core_network_id == core_network_id)
            .count()
    }

    /// How many Connect attachments are carried over the attachment `attachment_id` as
    /// their transport, for the attachment delete refusal.
    ///
    /// A Connect attachment is built on another attachment rather than on a resource of its
    /// own ([`AttachmentDetail::Connect::transport_attachment_id`]), so that transport is a
    /// dependent of exactly the kind every other delete of this service refuses to orphan.
    pub fn connect_attachment_count_over_transport(&self, attachment_id: &str) -> usize {
        self.attachments
            .values()
            .filter(|attachment| match &attachment.detail {
                AttachmentDetail::Connect {
                    transport_attachment_id,
                    ..
                } => transport_attachment_id == attachment_id,
                AttachmentDetail::Vpc { .. }
                | AttachmentDetail::SiteToSiteVpn
                | AttachmentDetail::DirectConnectGateway { .. }
                | AttachmentDetail::TransitGatewayRouteTable { .. } => false,
            })
            .count()
    }

    /// How many attachments are created over the peering `peering_id`, for the peering
    /// delete refusal.
    ///
    /// A transit gateway route table attachment takes its core network and its edge location
    /// from the peering it is created over
    /// ([`AttachmentDetail::TransitGatewayRouteTable::peering_id`]) rather than from a
    /// resource of its own, so that peering is a dependent of exactly the kind every other
    /// delete of this service refuses to orphan.
    pub fn attachment_count_over_peering(&self, peering_id: &str) -> usize {
        self.attachments
            .values()
            .filter(|attachment| match &attachment.detail {
                AttachmentDetail::TransitGatewayRouteTable {
                    peering_id: over, ..
                } => over == peering_id,
                AttachmentDetail::Connect { .. }
                | AttachmentDetail::Vpc { .. }
                | AttachmentDetail::SiteToSiteVpn
                | AttachmentDetail::DirectConnectGateway { .. } => false,
            })
            .count()
    }

    /// How many Connect peers are carried over the Connect attachment `attachment_id`, for
    /// the per-attachment quota [`MAX_CONNECT_PEERS_PER_ATTACHMENT`] and the attachment
    /// delete refusal: a peer hangs off exactly one Connect attachment, so deleting that
    /// attachment while it carries one would orphan the peer.
    pub fn connect_peer_count_of_attachment(&self, attachment_id: &str) -> usize {
        self.connect_peers
            .values()
            .filter(|peer| peer.connect_attachment_id == attachment_id)
            .count()
    }

    /// How many Connect peers the global network `global_network_id` holds, for the
    /// network-resource inventory: a peer hangs off a Connect attachment's core network,
    /// which hangs off a global network, so the count is resolved through that chain rather
    /// than stored twice.
    pub fn connect_peer_count_of_global_network(&self, global_network_id: &str) -> usize {
        self.connect_peers
            .values()
            .filter(|peer| self.holds_core_network(global_network_id, &peer.core_network_id))
            .count()
    }

    /// Whether the global network `global_network_id` owns the core network
    /// `core_network_id`: the chain every resource hanging off a core network is resolved to
    /// its global network through.
    pub(super) fn holds_core_network(&self, global_network_id: &str, core_network_id: &str) -> bool {
        self.core_networks
            .get(core_network_id)
            .is_some_and(|core_network| core_network.global_network_id == global_network_id)
    }

    /// How many attachments the global network `global_network_id` holds, for the
    /// network-resource inventory: an attachment hangs off a core network, which hangs off
    /// a global network, so the count is resolved through that chain rather than stored
    /// twice.
    pub fn attachment_count_of_global_network(&self, global_network_id: &str) -> usize {
        self.attachments
            .values()
            .filter(|attachment| {
                self.holds_core_network(global_network_id, &attachment.core_network_id)
            })
            .count()
    }

    /// How many peerings the global network `global_network_id` holds, for the
    /// network-resource inventory: a peering hangs off a core network, which hangs off a
    /// global network, so the count is resolved through that chain rather than stored
    /// twice.
    pub fn peering_count_of_global_network(&self, global_network_id: &str) -> usize {
        self.peerings_of_global_network(global_network_id).count()
    }

    /// The peerings the global network `global_network_id` holds, in `PeeringId` order.
    /// See [`Self::peering_count_of_global_network`] for the chain they are resolved
    /// through.
    pub(super) fn peerings_of_global_network<'a>(
        &'a self,
        global_network_id: &'a str,
    ) -> impl Iterator<Item = &'a PeeringRecord> {
        self.peerings.values().filter(move |peering| {
            self.core_networks
                .get(&peering.core_network_id)
                .is_some_and(|core_network| core_network.global_network_id == global_network_id)
        })
    }

    /// How many resources of `resource_type` the global network `global_network_id`
    /// holds, for the network-resource inventory.
    ///
    /// `resource_type` must be one of [`NETWORK_RESOURCE_TYPES`] (the caller refuses
    /// anything else). Only the kinds this service stores can be non-zero; a kind whose
    /// creating operation is still a scaffold stub is held nowhere, so the count reported
    /// for it is the zero the store really holds, not a placeholder.
    pub fn network_resource_count(&self, global_network_id: &str, resource_type: &str) -> i32 {
        let held = match resource_type {
            SITE_KIND => self.site_count_of(global_network_id),
            LINK_KIND => self.link_count_of_network(global_network_id),
            DEVICE_KIND => self.device_count_of(global_network_id),
            CONNECTION_KIND => self.connection_count_of(global_network_id),
            CORE_NETWORK_KIND => self.core_network_count_of(global_network_id),
            PEERING_KIND => self.peering_count_of_global_network(global_network_id),
            ATTACHMENT_KIND => self.attachment_count_of_global_network(global_network_id),
            CONNECT_PEER_KIND => self.connect_peer_count_of_global_network(global_network_id),
            _ => 0,
        };
        i32::try_from(held).unwrap_or(i32::MAX)
    }

    /// Every network resource the global network `global_network_id` holds, as of `now`,
    /// keyed by [`network_resource_key`] so the listing is ordered and a page token can
    /// name an offset into it.
    ///
    /// The inventory is the same one [`Self::network_resource_count`] counts, branch for
    /// branch — attachments, Connect peers, connections, core networks, devices, links and
    /// sites — so the count of a type and the resources listed for it can never disagree. A
    /// kind whose creating operation is still a scaffold stub is held nowhere and therefore
    /// contributes nothing, not a placeholder. A connection registered with a global
    /// network only for its telemetry lives in a different store
    /// ([`Self::telemetry_resources`]) that [`Self::network_resource_count`] does not count
    /// either, so the two agree on those registrations as well: a connection's session is
    /// reported by `GetNetworkTelemetry` alone, while the resource the registration hangs
    /// off — the Connect peer, or the attachment that attached the VPN connection — is what
    /// this inventory lists. A
    /// transit gateway registered with the global network
    /// ([`Self::transit_gateway_registrations`]) is a resource of Amazon VPC rather than one
    /// this service holds, so it is not part of this inventory either.
    pub fn network_resources_of(
        &self,
        global_network_id: &str,
        now: SystemTime,
    ) -> Vec<(String, NetworkResource)> {
        let mut resources: BTreeMap<String, NetworkResource> = BTreeMap::new();
        for record in self.attachments.values().filter(|attachment| {
            self.holds_core_network(global_network_id, &attachment.core_network_id)
        }) {
            // An attachment's Describe reader is the one for its OWN kind, so the
            // definition is rendered from that kind's shape — the same shape
            // `GetVpcAttachment` / `GetConnectAttachment` / `GetSiteToSiteVpnAttachment` /
            // `GetDirectConnectGatewayAttachment` answer. Each conversion answers `None`
            // for a record of another kind, so exactly one of them builds a shape here.
            let definition = if let Some(vpc) = record.to_vpc_attachment(now) {
                resource_definition(&VpcAttachment::SCHEMA, &vpc)
            } else if let Some(connect) = record.to_connect_attachment(now) {
                resource_definition(&ConnectAttachment::SCHEMA, &connect)
            } else if let Some(vpn) = record.to_site_to_site_vpn_attachment(now) {
                resource_definition(&SiteToSiteVpnAttachment::SCHEMA, &vpn)
            } else if let Some(gateway) = record.to_direct_connect_gateway_attachment(now) {
                resource_definition(&DirectConnectGatewayAttachment::SCHEMA, &gateway)
            } else if let Some(route_table) = record.to_transit_gateway_route_table_attachment(now)
            {
                resource_definition(&TransitGatewayRouteTableAttachment::SCHEMA, &route_table)
            } else {
                None
            };
            resources.insert(
                network_resource_key(ATTACHMENT_KIND, &record.id),
                NetworkResource {
                    // An attachment belongs to the core network it joins a resource to.
                    core_network_id: Some(record.core_network_id.clone()),
                    // The attachment lives at the edge it was created at, which its own
                    // reader reports as the Region of that edge
                    // ([`AttachmentRecord::sole_edge_location`]).
                    aws_region: record.sole_edge_location().map(str::to_string),
                    definition,
                    ..network_resource(ATTACHMENT_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self.peerings_of_global_network(global_network_id) {
            resources.insert(
                network_resource_key(PEERING_KIND, &record.id),
                NetworkResource {
                    // A peering belongs to the core network it joins to another resource.
                    core_network_id: Some(record.core_network_id.clone()),
                    // A peering joins ONE edge of its core network to the peer, and its
                    // own reader reports that edge as `Peering$EdgeLocation`.
                    aws_region: Some(record.edge_location.clone()),
                    definition: resource_definition(&Peering::SCHEMA, &record.to_peering(now)),
                    ..network_resource(PEERING_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .connect_peers
            .values()
            .filter(|peer| self.holds_core_network(global_network_id, &peer.core_network_id))
        {
            resources.insert(
                network_resource_key(CONNECT_PEER_KIND, &record.id),
                NetworkResource {
                    // A Connect peer belongs to the core network of the Connect attachment
                    // it is carried over.
                    core_network_id: Some(record.core_network_id.clone()),
                    // A Connect peer lands on the edge of that attachment, which its own
                    // reader reports as `ConnectPeer$EdgeLocation`.
                    aws_region: Some(record.edge_location.clone()),
                    definition: resource_definition(&ConnectPeer::SCHEMA, &record.to_shape(now)),
                    ..network_resource(CONNECT_PEER_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .connections
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            resources.insert(
                network_resource_key(CONNECTION_KIND, &record.id),
                NetworkResource {
                    definition: resource_definition(&Connection::SCHEMA, &record.to_shape(now)),
                    ..network_resource(CONNECTION_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .core_networks
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            resources.insert(
                network_resource_key(CORE_NETWORK_KIND, &record.id),
                NetworkResource {
                    // A core network IS a core network's resource, so this is the one
                    // kind of this service whose `CoreNetworkId` is its own id.
                    core_network_id: Some(record.id.clone()),
                    definition: resource_definition(&CoreNetwork::SCHEMA, &record.to_shape(now)),
                    ..network_resource(CORE_NETWORK_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .devices
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            resources.insert(
                network_resource_key(DEVICE_KIND, &record.id),
                NetworkResource {
                    definition: resource_definition(&Device::SCHEMA, &record.to_shape(now)),
                    ..network_resource(DEVICE_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .links
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            resources.insert(
                network_resource_key(LINK_KIND, &record.id),
                NetworkResource {
                    definition: resource_definition(&Link::SCHEMA, &record.to_shape(now)),
                    ..network_resource(LINK_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        for record in self
            .sites
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            resources.insert(
                network_resource_key(SITE_KIND, &record.id),
                NetworkResource {
                    definition: resource_definition(&Site::SCHEMA, &record.to_shape(now)),
                    ..network_resource(SITE_KIND, &record.id, &record.arn, &record.tags, now)
                },
            );
        }
        // "The resource metadata" (`smithy_query shape=NetworkResource`) is what
        // `UpdateNetworkResourceMetadata` stores for one resource ARN, so every resource of
        // this inventory reports the map last stored under its own ARN, whatever its kind,
        // and a resource nobody has stored metadata for reports none.
        for resource in resources.values_mut() {
            if let Some(stored) = resource
                .resource_arn
                .as_deref()
                .and_then(|arn| self.network_resource_metadata.get(arn))
            {
                resource.metadata = Some(metadata_shape(stored));
            }
        }
        resources.into_iter().collect()
    }

    /// Store `metadata` as the WHOLE metadata of the network resource `resource_arn` names
    /// inside the global network `global_network_id`, answering the map that resource then
    /// carries, or `None` when that global network holds no such network resource.
    ///
    /// The resource is resolved through the global network's own inventory
    /// ([`Self::network_resources_of`]), so exactly the resources a caller can SEE in that
    /// global network can be given metadata, and one naming another global network's
    /// resource — or none at all — is refused by the caller rather than stored.
    ///
    /// The map REPLACES whatever the resource carried: the operation's input carries the
    /// resource metadata in full, so a key the request leaves out is a key the resource no
    /// longer has.
    ///
    /// Writing also drops the entries of resources that have since been deleted, so the
    /// store holds metadata only for resources that still exist (see
    /// [`Self::network_resource_metadata`]).
    pub fn put_network_resource_metadata(
        &mut self,
        global_network_id: &str,
        resource_arn: &str,
        metadata: BTreeMap<String, String>,
        now: SystemTime,
    ) -> Option<BTreeMap<String, String>> {
        if !self
            .network_resources_of(global_network_id, now)
            .iter()
            .any(|(_, resource)| resource.resource_arn.as_deref() == Some(resource_arn))
        {
            return None;
        }
        let deleted: Vec<String> = self
            .network_resource_metadata
            .keys()
            .filter(|arn| self.tags_of(arn.as_str()).is_none())
            .cloned()
            .collect();
        for arn in deleted {
            self.network_resource_metadata.remove(&arn);
        }
        self.network_resource_metadata
            .insert(resource_arn.to_string(), metadata);
        self.network_resource_metadata.get(resource_arn).cloned()
    }

    /// Every relationship the global network `global_network_id` holds, keyed by
    /// [`relationship_key`] so the listing is ordered and a page token can name an offset
    /// into it.
    ///
    /// A relationship is one TIE the store holds between two resources named by ARN: the
    /// parent that owns a resource of this global network's inventory
    /// ([`Self::network_resources_of`]), the resource an attachment or a peering joins to
    /// it, and each resource an association ties it to. `From` is the resource the tie is
    /// recorded on — the child, the attaching resource, or, for an association that is no
    /// resource of its own, the DEVICE (and the LINK, where the association names one) of
    /// this global network it ties — and `To` is the resource at the other end. Every ARN
    /// comes from a stored record, so a tie whose other end belongs to another service (a
    /// customer gateway, a VPC, a transit gateway, a managed prefix list) is reported under
    /// the ARN that service's resource carries, and a pair two ties both name is held once.
    ///
    /// Two things deliberately contribute no pair:
    ///
    /// * A transit gateway registration ties a gateway to the GLOBAL NETWORK itself, which
    ///   is the scope this listing is taken over rather than a resource inside it.
    /// * The subnets a VPC attachment reaches its VPC through, and the subnet a device's
    ///   AWS location names, say HOW and WHERE a resource is reached rather than naming
    ///   another resource the tie joins — the resource an attachment attaches is the one
    ///   [`AttachmentRecord::resource_arn`] names.
    pub fn network_resource_relationships(
        &self,
        global_network_id: &str,
    ) -> Vec<(String, Relationship)> {
        /// Hold the pair `from` -> `to`. Keyed by the pair, so two stored ties that name
        /// the same pair leave one relationship rather than two.
        fn tie(pairs: &mut BTreeMap<String, Relationship>, from: &str, to: &str) {
            pairs.insert(
                relationship_key(from, to),
                Relationship {
                    from: Some(from.to_string()),
                    to: Some(to.to_string()),
                },
            );
        }

        // Each resolver answers the stored ARN of a resource of THIS global network, and
        // `None` for an id it holds none for — an id of another global network included, so
        // a tie never reaches out of the scope the listing is taken over.
        let site_arn = |id: &str| {
            self.sites
                .get(id)
                .filter(|record| record.global_network_id == global_network_id)
                .map(|record| record.arn.clone())
        };
        let link_arn = |id: &str| {
            self.links
                .get(id)
                .filter(|record| record.global_network_id == global_network_id)
                .map(|record| record.arn.clone())
        };
        let device_arn = |id: &str| {
            self.devices
                .get(id)
                .filter(|record| record.global_network_id == global_network_id)
                .map(|record| record.arn.clone())
        };
        let core_network_arn = |id: &str| {
            self.core_networks
                .get(id)
                .filter(|record| record.global_network_id == global_network_id)
                .map(|record| record.arn.clone())
        };
        let attachment_arn = |id: &str| {
            self.attachments
                .get(id)
                .filter(|record| self.holds_core_network(global_network_id, &record.core_network_id))
                .map(|record| record.arn.clone())
        };
        let peering_arn = |id: &str| {
            self.peerings
                .get(id)
                .filter(|record| self.holds_core_network(global_network_id, &record.core_network_id))
                .map(|record| record.arn.clone())
        };
        let connect_peer_arn = |id: &str| {
            self.connect_peers
                .get(id)
                .filter(|record| self.holds_core_network(global_network_id, &record.core_network_id))
                .map(|record| record.arn.clone())
        };

        let mut pairs: BTreeMap<String, Relationship> = BTreeMap::new();

        // A link belongs to a site, and a device sits at one where its create named one.
        for record in self
            .links
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            if let Some(site) = site_arn(&record.site_id) {
                tie(&mut pairs, &record.arn, &site);
            }
        }
        for record in self
            .devices
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            if let Some(site) = record.site_id.as_deref().and_then(site_arn) {
                tie(&mut pairs, &record.arn, &site);
            }
        }

        // A connection joins two devices of the global network, each over the link the
        // create named for it.
        for record in self
            .connections
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            for device in [&record.device_id, &record.connected_device_id] {
                if let Some(arn) = device_arn(device) {
                    tie(&mut pairs, &record.arn, &arn);
                }
            }
            for link in [record.link_id.as_deref(), record.connected_link_id.as_deref()]
                .into_iter()
                .flatten()
            {
                if let Some(arn) = link_arn(link) {
                    tie(&mut pairs, &record.arn, &arn);
                }
            }
        }

        // An attachment belongs to its core network and attaches one resource; a transit
        // gateway route table attachment reaches that route table THROUGH a peering, which
        // is the one kind whose detail names a further resource of its own.
        for record in self.attachments.values().filter(|record| {
            self.holds_core_network(global_network_id, &record.core_network_id)
        }) {
            tie(&mut pairs, &record.arn, &record.core_network_arn);
            tie(&mut pairs, &record.arn, &record.resource_arn);
            if let AttachmentDetail::TransitGatewayRouteTable { peering_id } = &record.detail {
                if let Some(arn) = peering_arn(peering_id) {
                    tie(&mut pairs, &record.arn, &arn);
                }
            }
        }

        // A peering joins its core network to the resource it peers with.
        for record in self.peerings_of_global_network(global_network_id) {
            tie(&mut pairs, &record.arn, &record.core_network_arn);
            tie(&mut pairs, &record.arn, &record.resource_arn);
        }

        // A Connect peer is carried over its Connect attachment.
        for record in self
            .connect_peers
            .values()
            .filter(|record| self.holds_core_network(global_network_id, &record.core_network_id))
        {
            if let Some(arn) = attachment_arn(&record.connect_attachment_id) {
                tie(&mut pairs, &record.arn, &arn);
            }
        }

        // A core network is tied to each managed prefix list associated with it.
        for record in self.prefix_list_associations.values() {
            if let Some(arn) = core_network_arn(&record.core_network_id) {
                tie(&mut pairs, &arn, &record.prefix_list_arn);
            }
        }

        // Every remaining tie is one an association holds between a resource and a DEVICE
        // of this global network, optionally over one of that device's LINKS: the tie is
        // recorded on each of those two, so each is a `From` of the resource it reaches.
        let mut device_ties: Vec<(&str, Option<&str>, String)> = Vec::new();
        for record in self
            .link_associations
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            if let Some(arn) = link_arn(&record.link_id) {
                device_ties.push((&record.device_id, None, arn));
            }
        }
        for record in self
            .customer_gateway_associations
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            device_ties.push((
                &record.device_id,
                record.link_id.as_deref(),
                record.customer_gateway_arn.clone(),
            ));
        }
        for record in self
            .connect_peer_associations
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            if let Some(arn) = connect_peer_arn(&record.connect_peer_id) {
                device_ties.push((&record.device_id, record.link_id.as_deref(), arn));
            }
        }
        for record in self
            .transit_gateway_connect_peer_associations
            .values()
            .filter(|record| record.global_network_id == global_network_id)
        {
            device_ties.push((
                &record.device_id,
                record.link_id.as_deref(),
                record.transit_gateway_connect_peer_arn.clone(),
            ));
        }
        for (device_id, link_id, to) in device_ties {
            if let Some(from) = device_arn(device_id) {
                tie(&mut pairs, &from, &to);
            }
            if let Some(from) = link_id.and_then(link_arn) {
                tie(&mut pairs, &from, &to);
            }
        }

        pairs.into_iter().collect()
    }

    /// Register `record` as a telemetry-bearing connection of its global network,
    /// replacing any registration that global network already holds for the same resource
    /// ARN (see [`telemetry_key`]).
    pub fn register_telemetry_resource(&mut self, record: TelemetryResourceRecord) {
        self.telemetry_resources.insert(
            telemetry_key(&record.global_network_id, &record.resource_arn),
            record,
        );
    }

    /// Register the connection the attachment `attachment_id` attaches as a
    /// telemetry-bearing connection of the global network that owns its core network, at
    /// `now`, answering whether a registration was made.
    ///
    /// The call that ties a connection into a global network is what registers its session
    /// ([`Self::register_telemetry_resource`], the same point the two Connect peer
    /// associates register theirs), so an attachment that attaches a connection whose
    /// session this service measures ([`AttachmentRecord::telemetry_resource_type`]) is
    /// registered by the call that starts attaching it: its create, or the acceptance that
    /// releases one the policy held for one. Nothing is registered for an attachment that
    /// attaches no such connection, for one still waiting for that acceptance or already
    /// rejected (nothing is created for it, so there is no session to measure:
    /// [`AttachmentRecord::is_created`]), or for one whose core network is no longer stored.
    ///
    /// The session starts DOWN and comes up when the attachment carrying it reaches
    /// `AVAILABLE` ([`AttachmentRecord::available_at`]) — the transition the clock already
    /// owes that attachment, so the session the registration starts needs no span of its
    /// own and no handler sleeps for it. Every member of the registration is read off the
    /// attachment: the connection's own ARN identifies it, the attachment's edge location is
    /// the Region the session lands in, and its core network is the one the connection
    /// reaches. `address` is left unset because a VPN connection's far end is Amazon VPC's
    /// record, which this service holds no inventory of.
    pub fn register_attachment_telemetry(&mut self, attachment_id: &str, now: SystemTime) -> bool {
        let Some(attachment) = self.attachments.get(attachment_id) else {
            return false;
        };
        let Some(resource_type) = attachment.telemetry_resource_type() else {
            return false;
        };
        let Some(connected_at) = attachment.available_at.filter(|_| attachment.is_created()) else {
            return false;
        };
        let Some(global_network_id) = self
            .core_networks
            .get(&attachment.core_network_id)
            .map(|core_network| core_network.global_network_id.clone())
        else {
            return false;
        };
        let record = TelemetryResourceRecord {
            global_network_id,
            resource_type: resource_type.to_string(),
            resource_arn: attachment.resource_arn.clone(),
            aws_region: attachment.edge_location.clone(),
            core_network_id: Some(attachment.core_network_id.clone()),
            // The connection reaches a core network edge rather than a gateway registered
            // with the global network, so no registration carries it.
            registered_gateway_arn: None,
            address: None,
            registered_at: now,
            connected_at,
        };
        self.register_telemetry_resource(record);
        true
    }

    /// Drop the registration the attachment `attachment` made for the connection it
    /// attaches, answering whether one was dropped.
    ///
    /// The counterpart of [`Self::register_attachment_telemetry`]: the call that unties the
    /// connection from the global network — the delete of the attachment that attached it —
    /// is what deregisters it, so a global network reports no session for a connection it
    /// no longer holds. Takes the record rather than its id, so the delete can untie an
    /// attachment it has already removed from the store.
    pub fn deregister_attachment_telemetry(&mut self, attachment: &AttachmentRecord) -> bool {
        if attachment.telemetry_resource_type().is_none() {
            return false;
        }
        let Some(global_network_id) = self
            .core_networks
            .get(&attachment.core_network_id)
            .map(|core_network| core_network.global_network_id.clone())
        else {
            return false;
        };
        self.deregister_telemetry_resource(&global_network_id, &attachment.resource_arn)
            .is_some()
    }

    /// Drop the registration `global_network_id` holds for the connection `resource_arn`,
    /// answering the record that was dropped, or `None` where it held none.
    ///
    /// The counterpart of [`Self::register_telemetry_resource`]: the operation that unties
    /// a connection from its global network is what deregisters it, so a global network
    /// stops reporting the session health of a peering it no longer holds.
    pub fn deregister_telemetry_resource(
        &mut self,
        global_network_id: &str,
        resource_arn: &str,
    ) -> Option<TelemetryResourceRecord> {
        self.telemetry_resources
            .remove(&telemetry_key(global_network_id, resource_arn))
    }

    /// Drop every record the store holds that exists only as a TIE to the Connect peer
    /// `connect_peer_id` — the association keyed by that peer
    /// ([`Self::connect_peer_associations`]) and the telemetry registration that
    /// association made under the peer's ARN in the global network it tied the peer to —
    /// answering the association that was dropped, or `None` where the peer held none.
    ///
    /// Removing the TIE (`DisassociateConnectPeer`) and removing the PEER
    /// (`DeleteConnectPeer`) both untie the peer through here, so the two records can never
    /// come apart: no reader of the ties reports an association for a peer that is gone,
    /// and no registration is left behind with nothing able to remove it.
    ///
    /// The ARN the registration is keyed by is computed from the id — the one name every
    /// Connect peer carries ([`Self::resource_arn`] over [`CONNECT_PEER_KIND`], which is
    /// how the create mints it) — rather than read out of the peer record, so untying works
    /// whether or not that record is still stored.
    pub fn untie_connect_peer(
        &mut self,
        connect_peer_id: &str,
    ) -> Option<ConnectPeerAssociationRecord> {
        let removed = self.connect_peer_associations.remove(connect_peer_id)?;
        self.deregister_telemetry_resource(
            &removed.global_network_id,
            &Self::resource_arn(CONNECT_PEER_KIND, connect_peer_id),
        );
        Some(removed)
    }

    /// Drop every record the store holds that exists only as a record OF the global network
    /// `global_network_id`, answering how many were dropped.
    ///
    /// Today that is its route analyses ([`Self::route_analyses`]): an analysis carries no
    /// identity outside the global network it was started on
    /// ([`RouteAnalysisRecord::global_network_id`], the only one `GetRouteAnalysis` resolves
    /// it through) and this service models no operation that removes one, so the analyses go
    /// with the global network the way the ties a core network holds go with the core
    /// network. Were they left behind, each would be keyed to a global network no call can
    /// address: unreadable, unremovable, and no longer counted by the per-global-network
    /// bound on the map.
    ///
    /// Unlike a dependent the caller can remove itself — a child resource, or a transit
    /// gateway registration — these cannot block the delete: there is no call that would
    /// empty them first.
    pub fn untie_global_network(&mut self, global_network_id: &str) -> usize {
        let held = self.route_analysis_count_of(global_network_id);
        self.route_analyses
            .retain(|_, analysis| analysis.global_network_id != global_network_id);
        held
    }

    /// How many route analyses the global network `global_network_id` holds, for the
    /// per-global-network quota ([`MAX_ROUTE_ANALYSES_PER_GLOBAL_NETWORK`]).
    pub fn route_analysis_count_of(&self, global_network_id: &str) -> usize {
        self.route_analyses
            .values()
            .filter(|analysis| analysis.global_network_id == global_network_id)
            .count()
    }

    /// What the global network `global_network_id` holds for one endpoint of a route
    /// analysis: the `member` the endpoint came from ([`ENDPOINT_SOURCE`] /
    /// [`ENDPOINT_DESTINATION`]) and the ARN and IP address it named.
    ///
    /// The endpoint's attachment ARN is looked up in the global network's one
    /// network-resource inventory ([`Self::network_resources_of`]) — the same inventory
    /// `GetNetworkResources` reports and `GetNetworkResourceCounts` counts — so a route
    /// analysis and that inventory cannot disagree about which attachments the global
    /// network holds. An attachment on a transit gateway is a resource of Amazon VPC, of
    /// which this service holds no inventory — registering the gateway with the global
    /// network records the registration and no attachment of it — so no such ARN resolves to
    /// one and the analysis reports the "cannot find an attachment" it really found rather
    /// than a placeholder.
    ///
    /// The endpoint's attachment ARN is an OPTIONAL reference member, so it is read through
    /// [`optional_reference`]: the empty string its modelled `len 0..` admits names no
    /// attachment, resolves nothing, and is held as none exactly as the omitted member is —
    /// so the stored endpoint options never echo back an ARN nothing can be stored under.
    pub(super) fn resolve_route_endpoint(
        &self,
        global_network_id: &str,
        member: &'static str,
        spec: &RouteAnalysisEndpointOptionsSpecification,
        now: SystemTime,
    ) -> RouteEndpoint {
        let attachment_arn = optional_reference(spec.transit_gateway_attachment_arn.clone());
        let resolution = match attachment_arn.as_deref() {
            None => RouteEndpointResolution::Unspecified,
            Some(arn) => {
                let held = self
                    .network_resources_of(global_network_id, now)
                    .into_iter()
                    .map(|(_, resource)| resource)
                    .find(|resource| resource.resource_arn.as_deref() == Some(arn));
                match held {
                    None => RouteEndpointResolution::Unknown,
                    Some(resource) => {
                        let resource_type = resource.resource_type.unwrap_or_default();
                        if resource_type == TRANSIT_GATEWAY_ATTACHMENT_TYPE {
                            RouteEndpointResolution::TransitGatewayAttachment {
                                // The gateway the attachment was registered through, which
                                // is the transit gateway it belongs to.
                                transit_gateway_arn: resource.registered_gateway_arn,
                            }
                        } else {
                            RouteEndpointResolution::OtherResource { resource_type }
                        }
                    }
                }
            }
        };
        RouteEndpoint {
            member,
            attachment_arn,
            ip_address: spec.ip_address.clone(),
            resolution,
        }
    }

    /// Start a route analysis on the global network `global_network_id` and answer the
    /// record it leaves behind.
    ///
    /// The analysis is run HERE, by the starting call, against what the global network
    /// holds at `now` ([`analyze_route_path`]): the paths are stored with the record, so
    /// every later read of the analysis reports the routing it found and nothing created
    /// afterwards changes it. The record is the whole answer — `StartRouteAnalysis` and
    /// `GetRouteAnalysis` both report it through [`RouteAnalysisRecord::to_shape`] — so the
    /// two cannot disagree about one analysis.
    pub fn start_route_analysis(
        &mut self,
        global_network_id: &str,
        source: &RouteAnalysisEndpointOptionsSpecification,
        destination: &RouteAnalysisEndpointOptionsSpecification,
        include_return_path: bool,
        use_middleboxes: bool,
        now: SystemTime,
    ) -> RouteAnalysisRecord {
        let source_endpoint =
            self.resolve_route_endpoint(global_network_id, ENDPOINT_SOURCE, source, now);
        let destination_endpoint =
            self.resolve_route_endpoint(global_network_id, ENDPOINT_DESTINATION, destination, now);
        let forward_path = analyze_route_path(&source_endpoint, &destination_endpoint);
        // "The return path is not analyzed if the forward path analysis does not succeed"
        // (`smithy_query query="doc" shape="RouteAnalysis" member="IncludeReturnPath"`), so
        // a forward path that did not reach the destination leaves the return path absent
        // even where the request asked for one.
        let return_path = (include_return_path && route_path_connected(&forward_path))
            .then(|| analyze_route_path(&destination_endpoint, &source_endpoint));
        let id = self.next_id(ROUTE_ANALYSIS_KIND);
        let record = RouteAnalysisRecord {
            id: id.clone(),
            global_network_id: global_network_id.to_string(),
            started_at: now,
            completed_at: now + SETTLE_DELAY,
            source: source_endpoint.options(),
            destination: destination_endpoint.options(),
            include_return_path,
            use_middleboxes,
            forward_path,
            return_path,
        };
        self.route_analyses.insert(id, record.clone());
        record
    }

    /// How many links `site_id` holds, for the per-site quota.
    pub fn link_count_of_site(&self, site_id: &str) -> usize {
        self.links
            .values()
            .filter(|link| link.site_id == site_id)
            .count()
    }

    /// The ids of the links attached to `site_id`, in `LinkId` order.
    pub fn link_ids_of_site(&self, site_id: &str) -> Vec<String> {
        self.links
            .values()
            .filter(|link| link.site_id == site_id)
            .map(|link| link.id.clone())
            .collect()
    }
}
