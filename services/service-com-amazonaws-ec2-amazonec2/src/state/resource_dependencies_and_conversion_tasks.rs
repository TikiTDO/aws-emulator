// SPDX-License-Identifier: CC0-1.0

//! What a resource's dependencies are — the default VPC of a Region a request that names
//! none is about, a VPC's default security group, the instances still holding a place in a
//! subnet — and the import/export conversion task stores.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

impl ComAmazonawsEc2Amazonec2State {
    /// The default VPC OF `region`, by id — the VPC a request that names none is about:
    /// "If you don't specify a security group when you launch an instance, the instance is
    /// launched into the appropriate default security group" (`CreateSecurityGroup`), the
    /// group of that VPC.
    ///
    /// Per REGION, because an account has one default VPC in each Region it calls in, and
    /// the VPC of another Region holds no subnet and no zone a request of THIS one could
    /// place anything in. The index is the one
    /// [`Self::ensure_default_network`] writes, and only a VPC the store still holds is
    /// answered — so a Region whose default VPC has been deleted answers `None`, the way an
    /// account that deleted its default VPC has none until it asks for a new one.
    pub fn default_vpc_id(&self, region: &str) -> Option<String> {
        self.default_vpc_ids
            .get(region)
            .filter(|vpc_id| self.vpcs.contains_key(vpc_id.as_str()))
            .cloned()
    }

    /// Whether `group_id` is the default security group of its VPC: the group named
    /// [`DEFAULT_SECURITY_GROUP_NAME`], which every VPC comes with and only `DeleteVpc`
    /// removes.
    pub fn is_default_security_group(&self, group_id: &str) -> bool {
        self.security_groups
            .get(group_id)
            .and_then(|group| group.group_name.as_deref())
            .is_some_and(|name| name == DEFAULT_SECURITY_GROUP_NAME)
    }

    /// The instances that still hold a place in `subnet_id`.
    ///
    /// "You must terminate all running instances in the subnet before you can delete the
    /// subnet" (`DeleteSubnet`) — an instance that has reached `terminated` holds nothing
    /// any more, so it is not a dependency; every other state is.
    pub fn instances_in_subnet(&self, subnet_id: &str) -> Vec<String> {
        let mut found: Vec<String> = self
            .instances
            .values()
            .filter(|instance| {
                instance.subnet_id.as_deref() == Some(subnet_id)
                    && instance.state != InstanceStateName::Terminated
            })
            .map(|instance| instance.instance_id.clone())
            .collect();
        found.sort();
        found
    }

    /// The network interfaces that still name the security group `group_id` among their
    /// `NetworkInterface$Groups`.
    ///
    /// The ONE definition of "an interface still holds that group", so every path that asks
    /// what a group is held by reads the same set.
    pub fn network_interfaces_with_security_group(&self, group_id: &str) -> Vec<String> {
        self.network_interfaces
            .iter()
            .filter(|(_, interface)| {
                interface
                    .groups
                    .iter()
                    .flatten()
                    .any(|group| group.group_id.as_deref() == Some(group_id))
            })
            .map(|(network_interface_id, _)| network_interface_id.clone())
            .collect()
    }

    /// The network interfaces of `vpc_id` that still name the security group `group_id`
    /// among their `NetworkInterface$Groups`.
    ///
    /// "You cannot disassociate the security group if any Elastic network interfaces in the
    /// associated VPC are still associated with the security group"
    /// (`DisassociateSecurityGroupVpc`) — what an interface naming the group IS stays
    /// [`Self::network_interfaces_with_security_group`]'s one definition, narrowed here to
    /// the VPC the interfaces live in.
    pub fn network_interfaces_in_vpc_with_security_group(
        &self,
        vpc_id: &str,
        group_id: &str,
    ) -> Vec<String> {
        self.network_interfaces_with_security_group(group_id)
            .into_iter()
            .filter(|network_interface_id| {
                self.network_interfaces
                    .get(network_interface_id)
                    .and_then(|interface| interface.vpc_id.as_deref())
                    == Some(vpc_id)
            })
            .collect()
    }

    /// The network interfaces that still hold a place in `subnet_id`.
    ///
    /// An interface is created in a subnet and names it for as long as it is stored, so it
    /// occupies that subnet — and the private addresses it took — until it is deleted.
    pub fn network_interfaces_in_subnet(&self, subnet_id: &str) -> Vec<String> {
        self.network_interfaces
            .iter()
            .filter(|(_, interface)| interface.subnet_id.as_deref() == Some(subnet_id))
            .map(|(network_interface_id, _)| network_interface_id.clone())
            .collect()
    }

    /// The EC2 Instance Connect Endpoints that still hold the network they were created in:
    /// the ones no delete has been asked for yet.
    ///
    /// A stored endpoint names its subnet, its VPC and its security groups for as long as it
    /// is stored, so it occupies them exactly as a stored network interface does — which is
    /// also how the real service accounts for one: "the ID of the elastic network interface
    /// that Amazon EC2 automatically created when creating the EC2 Instance Connect
    /// Endpoint" (`Ec2InstanceConnectEndpoint$NetworkInterfaceIds`). This crate holds no
    /// interface record for that interface, so the endpoint record is what the dependency is
    /// read from. Once its deletion HAS been asked for
    /// ([`Self::begin_instance_connect_endpoint_deletion`]) there is nothing left for the
    /// caller to take away: the endpoint is on its way out, and the record only stays behind
    /// to report the delete states the model gives it.
    pub(super) fn held_by_instance_connect_endpoints<Holds>(&self, holds: Holds) -> Vec<String>
    where
        Holds: Fn(&crate::ops::types::Ec2InstanceConnectEndpoint) -> bool,
    {
        self.instance_connect_endpoints
            .iter()
            .filter(|(_, record)| record.delete_complete_at.is_none() && holds(&record.view))
            .map(|(endpoint_id, _)| endpoint_id.clone())
            .collect()
    }

    /// The EC2 Instance Connect Endpoints that still hold a place in `subnet_id`.
    pub fn instance_connect_endpoints_in_subnet(&self, subnet_id: &str) -> Vec<String> {
        self.held_by_instance_connect_endpoints(|endpoint| {
            endpoint.subnet_id.as_deref() == Some(subnet_id)
        })
    }

    /// The EC2 Instance Connect Endpoints that still hold the security group `group_id`.
    pub fn instance_connect_endpoints_with_security_group(&self, group_id: &str) -> Vec<String> {
        self.held_by_instance_connect_endpoints(|endpoint| {
            endpoint
                .security_group_ids
                .iter()
                .flatten()
                .any(|held| held == group_id)
        })
    }

    /// The NAT gateways that still hold a place in `subnet_id`.
    ///
    /// "You must delete any NAT gateway in the subnet before you can delete the subnet" — a NAT
    /// gateway is created IN a subnet (`CreateNatGatewayRequest$SubnetId`) and holds an address
    /// of it for as long as it is there. One whose deletion has already been ASKED FOR holds
    /// nothing the caller still has to take away — the same reading
    /// [`Self::held_by_instance_connect_endpoints`] makes of an endpoint a delete has started
    /// on, and the one that lets a caller tear a network down in one pass — so the states that
    /// hold the subnet are the ones before that: every state but `deleting` and `deleted`.
    pub fn nat_gateways_in_subnet(&self, subnet_id: &str) -> Vec<String> {
        let mut found: Vec<String> = self
            .nat_gateways
            .iter()
            .filter(|(_, nat_gateway)| {
                nat_gateway.subnet_id.as_deref() == Some(subnet_id)
                    && !matches!(
                        nat_gateway.state.as_deref(),
                        Some(NAT_GATEWAY_STATE_DELETING | NAT_GATEWAY_STATE_DELETED)
                    )
            })
            .map(|(nat_gateway_id, _)| nat_gateway_id.clone())
            .collect();
        found.sort();
        found
    }

    /// What still depends on the subnet `subnet_id`, as the ids of the depending resources.
    ///
    /// "You must terminate all running instances in the subnet before you can delete the
    /// subnet" (`DeleteSubnet`), and a record created IN the subnet holds a place in it the
    /// same way an instance does — the delete is refused while any of them is still stored,
    /// rather than leaving a stored record naming a subnet that no longer exists.
    pub fn subnet_dependencies(&self, subnet_id: &str) -> Vec<String> {
        let mut found = self.instances_in_subnet(subnet_id);
        found.extend(self.network_interfaces_in_subnet(subnet_id));
        found.extend(self.instance_connect_endpoints_in_subnet(subnet_id));
        found.extend(self.nat_gateways_in_subnet(subnet_id));
        found.sort();
        found
    }

    /// What still depends on the security group `group_id`, as the ids of the depending
    /// resources.
    ///
    /// "If you attempt to delete a security group that is associated with an instance or
    /// network interface, is referenced by another security group in the same VPC, or has a
    /// VPC association, the operation fails with `DependencyViolation`"
    /// (`DeleteSecurityGroup`): an instance that holds the group, a network interface that
    /// holds it, and another group of the same VPC whose rules name it.
    pub fn security_group_dependencies(&self, group_id: &str) -> Vec<String> {
        let vpc_id = self
            .security_groups
            .get(group_id)
            .and_then(|group| group.vpc_id.clone());
        let mut found: Vec<String> = self
            .instances
            .values()
            .filter(|instance| {
                instance.state != InstanceStateName::Terminated
                    && instance
                        .security_groups
                        .iter()
                        .any(|(id, _)| id.as_deref() == Some(group_id))
            })
            .map(|instance| instance.instance_id.clone())
            .collect();
        // A network interface holds the groups it names for as long as the interface is
        // stored, attached to an instance or not: a stored interface is a reference to the
        // group, and there is no interface state that stops being one — so unlike an
        // instance, which lets go of its groups once it has reached `terminated`, an
        // interface is a dependency until it is deleted.
        found.extend(self.network_interfaces_with_security_group(group_id));
        // And so does an EC2 Instance Connect Endpoint, whose stored record names the groups
        // it was created with ([`Self::instance_connect_endpoints_with_security_group`]).
        found.extend(self.instance_connect_endpoints_with_security_group(group_id));
        for (other_id, other) in &self.security_groups {
            if other_id == group_id || other.vpc_id != vpc_id {
                continue;
            }
            let references = other
                .ip_permissions
                .iter()
                .chain(other.ip_permissions_egress.iter())
                .flatten()
                .flat_map(|permission| permission.user_id_group_pairs.iter().flatten())
                .any(|pair| pair.group_id.as_deref() == Some(group_id));
            if references {
                found.push(other_id.clone());
            }
        }
        found.sort();
        found
    }

    /// What still depends on the VPC `vpc_id`, as the ids of the depending resources.
    ///
    /// "You must detach or delete all gateways and resources that are associated with the
    /// VPC before you can delete it. For example, you must terminate all instances running
    /// in the VPC, delete all security groups associated with the VPC (except the default
    /// one) … When you delete the VPC, it deletes the default security group, network ACL,
    /// and route table for the VPC" (`DeleteVpc`). So the dependencies are this VPC's
    /// subnets, its security groups other than the default one, the network interfaces
    /// created in it and the instances still running in it; the default group is NOT one,
    /// because the delete removes it. A carrier gateway of the VPC is one too: it is a
    /// gateway "associated with the VPC", and `DeleteVpc` does not remove it.
    ///
    /// A route server ASSOCIATION is not one: a recorded `DeleteVpc` of a VPC a route
    /// server had been associated with succeeded, so the connection is not something the
    /// caller has to take away first — [`Self::remove_vpc`] takes it with the VPC instead.
    ///
    /// A record that sits in a SUBNET of the VPC rather than in the VPC itself — an EC2
    /// Instance Connect Endpoint ([`Self::instance_connect_endpoints_in_subnet`]) — needs no
    /// entry of its own: its subnet is a dependency of the VPC here, and that subnet cannot
    /// be deleted while the record is in it, so the VPC is held for as long as it is.
    pub fn vpc_dependencies(&self, vpc_id: &str) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        for (subnet_id, subnet) in &self.subnets {
            if subnet.vpc_id.as_deref() == Some(vpc_id) {
                found.push(subnet_id.clone());
            }
        }
        for (carrier_gateway_id, carrier_gateway) in &self.carrier_gateways {
            if carrier_gateway.vpc_id.as_deref() == Some(vpc_id) {
                found.push(carrier_gateway_id.clone());
            }
        }
        // An internet gateway still attached to the VPC is a gateway "associated with the
        // VPC" that the caller has to detach first ("You must detach or delete all gateways
        // and resources that are associated with the VPC before you can delete it" —
        // `DeleteVpc`), and the delete does not detach it.
        for (internet_gateway_id, internet_gateway) in &self.internet_gateways {
            if internet_gateway_attaches_vpc(internet_gateway, vpc_id) {
                found.push(internet_gateway_id.clone());
            }
        }
        for (group_id, group) in &self.security_groups {
            if group.vpc_id.as_deref() == Some(vpc_id)
                && group.group_name.as_deref() != Some(DEFAULT_SECURITY_GROUP_NAME)
            {
                found.push(group_id.clone());
            }
        }
        for (network_interface_id, interface) in &self.network_interfaces {
            if interface.vpc_id.as_deref() == Some(vpc_id) {
                found.push(network_interface_id.clone());
            }
        }
        for instance in self.instances.values() {
            if instance.state == InstanceStateName::Terminated {
                continue;
            }
            if self.instance_is_in_vpc(instance, vpc_id) {
                found.push(instance.instance_id.clone());
            }
        }
        found.sort();
        found
    }

    /// Whether `instance` sits in the VPC `vpc_id`: its subnet is one of that VPC's.
    ///
    /// An instance's network placement is its subnet and nothing else, so the VPC is
    /// resolved through the subnet store — by [`Self::instance_vpc_id`], the ONE reading of
    /// it, so this and the instance report cannot disagree. [`Self::vpc_dependencies`] asks
    /// this to decide what a `DeleteVpc` is blocked by, and
    /// `GetVpcResourcesBlockingEncryptionEnforcement` asks it to decide whose instances a
    /// report covers.
    ///
    /// The instance's STATE is not read here: which states still count as occupying the VPC
    /// is the caller's question (a delete is blocked by an instance that is not terminated;
    /// a report covers the same set), so each caller states it where it asks.
    pub fn instance_is_in_vpc(&self, instance: &InstanceRecord, vpc_id: &str) -> bool {
        self.instance_vpc_id(instance).as_deref() == Some(vpc_id)
    }

    /// Remove the subnet `subnet_id` and answer it as it was last reported, or `None` when
    /// the store holds no such subnet. The bytes the record held go back to the store's
    /// budget, and every transition the clock may still owe it — its `available` move, its
    /// IPv6 association's `associated` one, and the `disassociated` one owed to an
    /// association it is part way through letting go of — goes with it, so a later
    /// materialisation cannot resurrect a deleted subnet's state.
    pub fn remove_subnet(&mut self, subnet_id: &str) -> Option<Subnet> {
        let subnet = self.subnets.remove(subnet_id)?;
        self.subnet_available_at.remove(subnet_id);
        self.subnet_ipv6_associated_at.remove(subnet_id);
        // A move owed to an association OF THIS SUBNET is owed no longer: the association
        // went with the subnet, and the bytes it retained are given back below with the rest
        // of the record's.
        for association in subnet.ipv6_cidr_block_association_set.iter().flatten() {
            if let Some(association_id) = association.association_id.as_deref() {
                self.subnet_ipv6_disassociated_at.remove(association_id);
            }
        }
        // An association between a network ACL and THIS subnet cannot outlive the subnet:
        // it goes here rather than being left behind for `DescribeNetworkAcls` to report an
        // ACL associated with a subnet that no longer exists.
        self.disassociate_subnet_from_network_acls(subnet_id);
        // An association between a route table and THIS subnet cannot outlive the subnet
        // either, on the same terms: an association is the pair, so it goes here rather than
        // being left behind for `DescribeRouteTables` to report a route table associated
        // with a subnet that no longer exists.
        self.disassociate_subnet_from_route_tables(subnet_id);
        self.release_stored_bytes(subnet_payload_bytes(&subnet));
        Some(subnet)
    }

    /// Remove the security group `group_id` and answer it as it was last reported, or
    /// `None` when the store holds no such group.
    ///
    /// The bytes the record held go back to the store's budget on exactly the terms they
    /// were taken on: `CreateSecurityGroup` charged [`security_group_payload_bytes`] of the
    /// group it stored, while the default group of a VPC carries a name and description the
    /// SERVICE chose and was never charged for them — so releasing for it would walk
    /// `stored_bytes` below what the store actually holds and silently weaken
    /// [`MAX_STORED_BYTES`]. A caller cannot create a second group named
    /// [`DEFAULT_SECURITY_GROUP_NAME`] in a VPC (the name is already taken), so that name
    /// identifies the uncharged group exactly.
    ///
    /// The group's RULES go with it — a rule of a group that no longer exists is a rule of
    /// nothing — and the bytes each of them retained go back to the budget they were taken
    /// from when the rule was stored.
    pub fn remove_security_group(&mut self, group_id: &str) -> Option<SecurityGroup> {
        let was_default = self.is_default_security_group(group_id);
        let group = self.security_groups.remove(group_id)?;
        // The group's rules are resources of the group: a rule can only be named through
        // the group it belongs to, so none may outlive it in the store. The bytes each one
        // was charged go back to the budget, taken from the record itself so a removal gives
        // back exactly what the insert took.
        let rule_ids: Vec<String> = self
            .security_group_rules
            .iter()
            .filter(|(_, record)| record.rule.group_id.as_deref() == Some(group_id))
            .map(|(rule_id, _)| rule_id.clone())
            .collect();
        for rule_id in rule_ids {
            if let Some(record) = self.security_group_rules.remove(&rule_id) {
                self.release_stored_bytes(record.payload_bytes);
            }
        }
        if !was_default {
            self.release_stored_bytes(security_group_payload_bytes(&group));
        }
        // An association IS this group being usable in another VPC
        // (`SecurityGroupVpcAssociation`), so it cannot outlive the group: it goes here
        // rather than being left behind for `DescribeSecurityGroupVpcAssociations` to
        // report a group that no longer exists. The record holds no caller-supplied bytes,
        // so there is nothing to give back to the budget.
        self.security_group_vpc_associations
            .retain(|(associated_group, _), _| associated_group != group_id);
        Some(group)
    }

    /// Remove the VPC `vpc_id` with the resources its delete takes with it, and answer it
    /// as it was last reported, or `None` when the store holds no such VPC.
    ///
    /// "When you delete the VPC, it deletes the default security group, network ACL, and
    /// route table for the VPC" (`DeleteVpc`), so the groups of this VPC go here rather
    /// than being left behind naming a VPC that no longer exists — the caller has already
    /// had to delete every other one ([`Self::vpc_dependencies`]). The bytes every removed
    /// record held go back to the store's budget, and the `available` transition the clock
    /// may still owe the VPC goes with it.
    pub fn remove_vpc(&mut self, vpc_id: &str) -> Option<Vpc> {
        let vpc = self.vpcs.remove(vpc_id)?;
        self.vpc_available_at.remove(vpc_id);
        // The Region index entry goes with the VPC as well, so the map cannot outlive the
        // VPCs it places and a VPC id this store no longer holds places nothing.
        self.vpc_regions.remove(vpc_id);
        // The attributes `insert_vpc` wrote for this VPC go with it as well: a VPC created
        // later must start from the creation defaults, never from what a
        // `ModifyVpcAttribute` left behind for a VPC that no longer exists.
        self.vpc_attributes.remove(vpc_id);
        // The ClassicLink enablement of this VPC goes with it too: a VPC created later must
        // start out not enabled for ClassicLink, never from what an enable left behind for a
        // VPC that no longer exists.
        self.classic_link_enabled_vpcs.remove(vpc_id);
        // And so does its ClassicLink DNS support, on the same terms: the two enablements are
        // separate facts of the VPC, and neither may outlive it.
        self.classic_link_dns_enabled_vpcs.remove(vpc_id);
        // The Region's DHCP option set does NOT go with the VPC: one set is shared by every
        // VPC of the Region and outlives all of them, as it does on AWS, so a delete leaves
        // [`Self::dhcp_options`] alone.
        self.release_stored_bytes(vpc_payload_bytes(&vpc));
        let groups: Vec<String> = self
            .security_groups
            .iter()
            .filter(|(_, group)| group.vpc_id.as_deref() == Some(vpc_id))
            .map(|(group_id, _)| group_id.clone())
            .collect();
        for group_id in groups {
            self.remove_security_group(&group_id);
        }
        // The network ACLs of this VPC go with it on exactly the same terms as the groups
        // above, and for the same documented reason: "when you delete the VPC, it deletes
        // the default security group, network ACL, and route table for the VPC"
        // (`DeleteVpc`). An ACL names the VPC it filters, so none may be left behind for a
        // describe to report a VPC that no longer exists.
        let network_acl_ids: Vec<String> = self
            .network_acls
            .iter()
            .filter(|(_, acl)| acl.vpc_id.as_deref() == Some(vpc_id))
            .map(|(network_acl_id, _)| network_acl_id.clone())
            .collect();
        for network_acl_id in network_acl_ids {
            self.remove_network_acl(&network_acl_id);
        }
        // The route tables of this VPC go with it on the same documented terms: "when you
        // delete the VPC, it deletes the default security group, network ACL, and route
        // table for the VPC" (`DeleteVpc`). A route table names the VPC it routes for, so
        // none may be left behind for `DescribeRouteTables` to report a VPC that no longer
        // exists — the caller has already had to detach every gateway
        // ([`Self::vpc_dependencies`]).
        let route_table_ids: Vec<String> = self
            .route_tables
            .iter()
            .filter(|(_, route_table)| route_table.vpc_id.as_deref() == Some(vpc_id))
            .map(|(route_table_id, _)| route_table_id.clone())
            .collect();
        for route_table_id in route_table_ids {
            self.remove_route_table(&route_table_id);
        }
        // A route server association IS the connection between a route server and THIS VPC
        // (`RouteServerAssociation`), so it cannot outlive the VPC: it goes here rather than
        // being left behind for `GetRouteServerAssociations` to report a VPC that no longer
        // exists. The record holds no caller-supplied bytes, so there is nothing to give
        // back to the budget.
        self.route_server_associations
            .retain(|(_, associated_vpc), _| associated_vpc != vpc_id);
        // A security group VPC association names THIS VPC as the VPC the group is usable
        // in, so it cannot outlive it either — on the same terms as the route server
        // associations above. The associations of the groups removed with the VPC have
        // already gone with those groups.
        self.security_group_vpc_associations
            .retain(|(_, associated_vpc), _| associated_vpc != vpc_id);
        Some(vpc)
    }
}

// ---------------------------------------------------------------------------
// VM Import/Export snapshot import tasks
// ---------------------------------------------------------------------------
//
// An import snapshot task is the record of ONE `ImportSnapshot` request: the disk image
// it names, the snapshot being made from it, and how far that has got. It is stored as
// the modelled `ImportSnapshotTask` shape
// ([`ComAmazonawsEc2Amazonec2State::import_snapshot_tasks`]) for the same reason a VPC
// is stored as a `Vpc`: `DescribeImportSnapshotTasks` reports exactly what a writer
// stored, so the two cannot disagree about one task.
//
// The task's state lives on its `SnapshotTaskDetail$Status`, which is the only place
// either operation reads it from, and the `task-state` filter selects on. Those state
// values, and `INVALID_CONVERSION_TASK_ID`, are the ones the import IMAGE task store above
// already declares — an import task is an import task, whichever resource it produces — so
// nothing here redeclares them.

/// The most import snapshot tasks this in-memory store holds at once.
///
/// A capacity bound in the same style as [`MAX_STORED_IMPORT_IMAGE_TASKS`], not an AWS
/// quota: each task is a record in this process's memory, so the number a caller can create
/// has to be finite.
/// [`ComAmazonawsEc2Amazonec2State::has_import_snapshot_task_capacity`] is the predicate a
/// writer asks first, and
/// [`ComAmazonawsEc2Amazonec2State::insert_import_snapshot_task`] refuses past it so no
/// path can grow the store without bound even if a writer forgets to ask.
pub const MAX_STORED_IMPORT_SNAPSHOT_TASKS: usize = 1_000;

/// What a writer of an import snapshot task supplies: everything the task carries that
/// comes from the REQUEST, with the task's own id left to
/// [`ComAmazonawsEc2Amazonec2State::insert_import_snapshot_task`] to mint.
#[derive(Debug, Clone, Default)]
pub struct ImportSnapshotTaskSpec {
    /// "The description string for the import snapshot task"
    /// (`ImportSnapshotRequest$Description`), which the task reports on both its own
    /// `Description` and its detail's.
    pub description: Option<String>,
    /// The progress of the import itself: the disk image it reads, the snapshot it makes,
    /// the status it is in and how far it has got.
    pub detail: SnapshotTaskDetail,
    /// "The tags to apply to the import snapshot task during creation"
    /// (`ImportSnapshotRequest$TagSpecifications`).
    pub tags: Vec<Tag>,
}

/// The bytes a stored import snapshot task holds: [`IMPORT_TASK_ELEMENT_OVERHEAD_BYTES`] of
/// per-record overhead — the same floor the import image task store charges, so a task
/// carrying no text at all is not free against [`MAX_STORED_BYTES`] — plus its tags and the
/// text of the request it was created from. The members the SERVICE owns (the task id, the
/// snapshot id, the status, the progress) are fixed-size and stand for by the overhead.
pub fn import_snapshot_task_payload_bytes(task: &ImportSnapshotTask) -> usize {
    let detail = task.snapshot_task_detail.as_ref();
    let text = |value: Option<&String>| value.map_or(0, String::len);
    let bucket = detail.and_then(|detail| detail.user_bucket.as_ref());
    tag_list_bytes(task.tags.as_deref().unwrap_or_default())
        .saturating_add(IMPORT_TASK_ELEMENT_OVERHEAD_BYTES)
        .saturating_add(text(task.description.as_ref()))
        .saturating_add(text(detail.and_then(|detail| detail.description.as_ref())))
        .saturating_add(text(detail.and_then(|detail| detail.format.as_ref())))
        .saturating_add(text(detail.and_then(|detail| detail.kms_key_id.as_ref())))
        .saturating_add(text(detail.and_then(|detail| detail.url.as_ref())))
        .saturating_add(text(bucket.and_then(|bucket| bucket.s3_bucket.as_ref())))
        .saturating_add(text(bucket.and_then(|bucket| bucket.s3_key.as_ref())))
}

/// The prefix, with its hyphen, of an import SNAPSHOT task id.
pub const IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN: &str = "import-snap-";

/// Whether `id` is a well-formed import SNAPSHOT task id:
/// [`import_task_id_is_well_formed`] — the one reader of an import task id's form — on the
/// `import-snap-` family.
pub fn import_snapshot_task_id_is_well_formed(id: &str) -> bool {
    import_task_id_is_well_formed(IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN, id)
}

/// Refuse an `ImportTaskIds` entry that is not an import snapshot task id at all: HTTP 400
/// [`INVALID_CONVERSION_TASK_ID`], EC2's code for a conversion task id it cannot resolve.
///
/// Only a MALFORMED id is refused, the same rule
/// [`require_well_formed_import_image_task_id`] holds for the sibling describe: a
/// well-formed id no task of the account has is not an error, because
/// `DescribeImportSnapshotTasks` answers "a list of zero or more import snapshot tasks"
/// (`DescribeImportSnapshotTasksResult$ImportSnapshotTasks`), so such an id simply selects
/// no row.
pub fn require_well_formed_import_snapshot_task_id<E>(
    task_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    require_well_formed_import_task_id(IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN, task_id)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another import snapshot task fits inside the store's ceiling
    /// ([`MAX_STORED_IMPORT_SNAPSHOT_TASKS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a writer asks first and refuses
    /// rather than growing the store past its ceiling.
    pub fn has_import_snapshot_task_capacity(&self) -> bool {
        self.import_snapshot_tasks.len() < MAX_STORED_IMPORT_SNAPSHOT_TASKS
    }

    /// Store an import snapshot task, and answer the `ImportTaskId` it was given — or
    /// `None` when the store already holds [`MAX_STORED_IMPORT_SNAPSHOT_TASKS`] of them, in
    /// which case NOTHING is stored and no bytes are charged.
    ///
    /// The ceiling is enforced here as well as asked about through
    /// [`Self::has_import_snapshot_task_capacity`], because the count is the only thing
    /// bounding this store's growth: a task's byte charge is dominated by caller-chosen
    /// text it need not carry at all, so the byte budget alone cannot stop a sequence of
    /// bare records from growing the store without bound.
    ///
    /// The id is minted here rather than taken from the caller, so a task never shares an
    /// id with another resource of this store, and the description the request carried is
    /// written onto BOTH the task (`ImportSnapshotTask$Description`, "A description of the
    /// import snapshot task") and its detail (`SnapshotTaskDetail$Description`, "The
    /// description of the disk image being imported") — one request field, so two readers
    /// cannot see two descriptions. The caller-chosen bytes are charged against the
    /// store's byte budget HERE rather than by each writer, the way [`Self::insert_vpc`]
    /// charges a VPC's tags.
    pub fn insert_import_snapshot_task(
        &mut self,
        spec: ImportSnapshotTaskSpec,
    ) -> Option<String> {
        if !self.has_import_snapshot_task_capacity() {
            return None;
        }
        let import_task_id = self.next_id(
            IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN.trim_end_matches('-'),
        );
        let task = ImportSnapshotTask {
            description: spec.description.clone(),
            import_task_id: Some(import_task_id.clone()),
            snapshot_task_detail: Some(SnapshotTaskDetail {
                description: spec.description,
                ..spec.detail
            }),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(import_snapshot_task_payload_bytes(&task));
        self.import_snapshot_tasks
            .insert(import_task_id.clone(), task);
        Some(import_task_id)
    }
}

// ---------------------------------------------------------------------------
// What the clock owes an import snapshot task
// ---------------------------------------------------------------------------
//
// "Imports a disk into an EBS snapshot" (`ImportSnapshot`): the task a caller is handed
// back is `active`, and the snapshot that disk becomes is owed by the CLOCK rather than by
// a later call. The window that takes is kept BESIDE the stored task
// ([`ComAmazonawsEc2Amazonec2State::import_snapshot_conversions`]) for the reason
// [`ComAmazonawsEc2Amazonec2State::import_task_deleted_at`] is: the task is stored as the
// modelled shape its describe reports, and `SnapshotTaskDetail` carries no member for an
// instant. A task written with no window — one seeded in a state the import did not put it
// in — is therefore left exactly as stored.

/// The disk image formats an `ImportSnapshot` disk container may carry: "Valid values:
/// VHD | VMDK | RAW" (`SnapshotDiskContainer$Format`).
///
/// A SHORTER catalog than the one a whole-machine import takes
/// ([`IMPORT_DISK_IMAGE_FORMATS`]), which also admits the container formats a VM's disks
/// arrive in: what this operation imports is one bare disk image.
pub const IMPORT_SNAPSHOT_DISK_IMAGE_FORMATS: [&str; 3] = ["VHD", "VMDK", "RAW"];

/// The resource type an `ImportSnapshot` `TagSpecification` may name: "The tags to apply to
/// the import snapshot task during creation" (`ImportSnapshotRequest$TagSpecifications`),
/// which the modelled `ResourceType` enum spells `import-snapshot-task`.
pub const IMPORT_SNAPSHOT_TASK_TAGGABLE_RESOURCE: &str = "import-snapshot-task";

/// The status message an import snapshot task carries while the disk image it names is
/// still being read: the import has been accepted and the snapshot it makes does not exist
/// yet, which is a different fact from the `active` state on its own
/// (`SnapshotTaskDetail$StatusMessage`, "A detailed status message for the import snapshot
/// task").
pub const IMPORT_SNAPSHOT_TASK_STATUS_MESSAGE_PENDING: &str = "pending";

/// The window one import snapshot task's conversion runs over: when it started, when the
/// snapshot it makes is minted, and when the task may report itself `completed`.
///
/// Every instant is monotonic ([`Instant`]), because each is only ever compared against the
/// clock and never reported, so a wall-clock adjustment cannot move a task back to `active`.
#[derive(Debug, Clone)]
pub struct ImportSnapshotConversion {
    /// When the import started, which its progress percentage is measured from.
    pub started_at: Instant,
    /// The instant from which the snapshot this import makes is due to exist. Before it,
    /// the task reports no `SnapshotId`: there is no snapshot of a disk that has not been
    /// read yet.
    pub snapshot_due_at: Instant,
    /// The instant from which the task's `active` -> `completed` move is due, which is the
    /// instant the snapshot it minted is itself `completed` — a task that provisions another
    /// resource is not finished before that resource is ready. `None` until the snapshot has
    /// been minted, because until then there is nothing to wait for.
    pub completed_at: Option<Instant>,
}

impl ImportSnapshotConversion {
    /// The instant the import's progress percentage runs out at: the end of the whole
    /// window, which is known exactly once the snapshot has been minted and is one further
    /// [`TRANSITION_DELAY`] away before that.
    pub fn ends_at(&self) -> Instant {
        self.completed_at
            .unwrap_or(self.snapshot_due_at + TRANSITION_DELAY)
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Record that the import snapshot task `import_task_id` started converting its disk at
    /// `now`, so the clock owes it the snapshot and the completion
    /// [`Self::complete_due_import_snapshot_conversions`] applies.
    ///
    /// Separate from [`Self::insert_import_snapshot_task`] because a task can be STORED
    /// without an import running behind it — one written directly in a terminal state has
    /// nothing left for the clock to do — and only the operation that starts an import knows
    /// that it did.
    pub fn start_import_snapshot_conversion(&mut self, import_task_id: &str, now: Instant) {
        self.import_snapshot_conversions.insert(
            import_task_id.to_string(),
            ImportSnapshotConversion {
                started_at: now,
                snapshot_due_at: now + TRANSITION_DELAY,
                completed_at: None,
            },
        );
    }

    /// `task` as of `now`: the stored task with the percentage of progress an import still
    /// converting has made by then. A read observes that percentage without writing
    /// anything, for the reason [`ImportImageTaskRecord::view_at`] derives the same member —
    /// progress is a fact of elapsed time, so no stored value could be right twice.
    ///
    /// A task with no conversion window behind it, and one that is no longer `active`, is
    /// answered exactly as stored: neither has a percentage left to make.
    pub fn import_snapshot_task_view(
        &self,
        task: &ImportSnapshotTask,
        now: Instant,
    ) -> ImportSnapshotTask {
        let mut view = task.clone();
        let active = view
            .snapshot_task_detail
            .as_ref()
            .and_then(|detail| detail.status.as_deref())
            == Some(IMPORT_TASK_STATE_ACTIVE);
        let window = view
            .import_task_id
            .as_deref()
            .and_then(|task_id| self.import_snapshot_conversions.get(task_id))
            .map(|conversion| (conversion.started_at, conversion.ends_at()));
        if let (true, Some((started_at, ends_at))) = (active, window) {
            let progress = import_task_progress_at(started_at, ends_at, now);
            if let Some(detail) = view.snapshot_task_detail.as_mut() {
                detail.progress = Some(progress);
            }
        }
        view
    }

    /// The import snapshot task `import_task_id` as of `now`, or `None` when this account
    /// holds no import snapshot task under that id — [`Self::import_snapshot_task_view`] on
    /// one task the store names.
    pub fn import_snapshot_task_view_at(
        &self,
        import_task_id: &str,
        now: Instant,
    ) -> Option<ImportSnapshotTask> {
        let task = self.import_snapshot_tasks.get(import_task_id)?;
        Some(self.import_snapshot_task_view(task, now))
    }

    /// The import snapshot task `import_task_id` as `ImportSnapshot` reports it:
    /// [`Self::import_snapshot_task_view_at`] through the members `ImportSnapshotResult`
    /// declares, which are the task's own.
    pub fn import_snapshot_response_at(
        &self,
        import_task_id: &str,
        now: Instant,
    ) -> Option<ImportSnapshotOutput> {
        let view = self.import_snapshot_task_view_at(import_task_id, now)?;
        Some(ImportSnapshotOutput {
            description: view.description,
            import_task_id: view.import_task_id,
            snapshot_task_detail: view.snapshot_task_detail,
            tags: view.tags,
        })
    }

    /// Carry every import snapshot task the clock owes a move to the state it is owed: the
    /// snapshot its disk becomes once the conversion window has passed, and the
    /// `active` -> `completed` move once that snapshot is itself `completed`.
    ///
    /// Reached only through [`Self::materialize_due_transitions`], so a reporting operation
    /// observes both moves without having caused either: each was owed at this instant
    /// whether or not anybody called. They are WRITES rather than derives because the first
    /// of them MINTS a resource — "the snapshot ID of the disk being imported"
    /// (`SnapshotTaskDetail$SnapshotId`), an EBS snapshot of this account's own store, which
    /// every later read of either resource has to agree on.
    ///
    /// The task waits for the snapshot to be `completed` before reporting itself completed,
    /// because an import that has not finished writing the snapshot it made has not
    /// finished. A cancel that got in first takes the task out of `active`, and its
    /// conversion then produces nothing at all.
    pub fn complete_due_import_snapshot_conversions(&mut self, now: Instant) {
        let converting: Vec<String> = self
            .import_snapshot_conversions
            .iter()
            .filter(|(_, conversion)| {
                conversion.completed_at.is_none() && now >= conversion.snapshot_due_at
            })
            .map(|(import_task_id, _)| import_task_id.clone())
            .collect();
        for import_task_id in converting {
            if self.import_task_state(&import_task_id).as_deref() != Some(IMPORT_TASK_STATE_ACTIVE)
            {
                // The task is gone or no longer importing — a cancel reached it first — so
                // there is no disk being read and nothing more is owed on it.
                self.import_snapshot_conversions.remove(&import_task_id);
                continue;
            }
            let Some(view) = self.imported_snapshot_view(&import_task_id) else {
                self.import_snapshot_conversions.remove(&import_task_id);
                continue;
            };
            // No room for the snapshot means the import cannot produce one at all, so the
            // task ends where a failed import ends rather than staying `active` for ever,
            // and its message says which ceiling stopped it — the number of snapshots this
            // store holds, or the caller-chosen bytes they hold, a count of records being no
            // bound on the bytes each one carries. The same rule
            // [`Self::complete_due_import_image_tasks`] holds for the AMI its own import
            // could not create.
            let no_room = if !self.has_snapshot_capacity(1) {
                Some(format!(
                    "This emulator holds {MAX_STORED_SNAPSHOTS} EBS snapshots, which is as \
                     many as it stores, so the imported snapshot could not be created"
                ))
            } else {
                let payload_bytes = snapshot_payload_bytes(
                    view.tags.as_deref().unwrap_or_default(),
                    view.description.as_deref(),
                );
                self.would_exceed_stored_bytes(payload_bytes).then(|| {
                    format!(
                        "This emulator holds {held} of at most {MAX_STORED_BYTES} bytes of \
                         caller-supplied data, so the imported snapshot could not be created \
                         with {payload_bytes} more",
                        held = self.stored_bytes()
                    )
                })
            };
            if let Some(reason) = no_room {
                if let Some(detail) = self
                    .import_snapshot_tasks
                    .get_mut(&import_task_id)
                    .and_then(|task| task.snapshot_task_detail.as_mut())
                {
                    detail.status = Some(IMPORT_TASK_STATE_DELETED.to_string());
                    detail.status_message = Some(reason);
                    detail.progress = None;
                }
                self.import_snapshot_conversions.remove(&import_task_id);
                continue;
            }
            // The snapshot the import creates is an EBS snapshot of this account like any
            // other, so it is stored through the ONE writer of one rather than beside it —
            // which is also what gives it the `pending` -> `completed` move every snapshot
            // starts out owing. That move is anchored to the instant the conversion fell
            // DUE, not to the instant this pass ran, so when the import finishes is fixed by
            // the call that started it rather than by when anybody first looked.
            let snapshot_due_at = self
                .import_snapshot_conversions
                .get(&import_task_id)
                .map_or(now, |conversion| conversion.snapshot_due_at);
            let snapshot_completed_at = snapshot_due_at + TRANSITION_DELAY;
            let snapshot_id =
                self.insert_snapshot_completing_at(view, SystemTime::now(), snapshot_completed_at);
            if let Some(detail) = self
                .import_snapshot_tasks
                .get_mut(&import_task_id)
                .and_then(|task| task.snapshot_task_detail.as_mut())
            {
                detail.snapshot_id = Some(snapshot_id);
            }
            if let Some(conversion) = self.import_snapshot_conversions.get_mut(&import_task_id) {
                conversion.completed_at = Some(snapshot_completed_at);
            }
        }

        let completed: Vec<String> = self
            .import_snapshot_conversions
            .iter()
            .filter(|(_, conversion)| conversion.completed_at.is_some_and(|due| now >= due))
            .map(|(import_task_id, _)| import_task_id.clone())
            .collect();
        for import_task_id in completed {
            if let Some(detail) = self
                .import_snapshot_tasks
                .get_mut(&import_task_id)
                .and_then(|task| task.snapshot_task_detail.as_mut())
            {
                // Only a task still importing completes: one a cancel moved on keeps the
                // state that cancel left it in.
                if detail.status.as_deref() == Some(IMPORT_TASK_STATE_ACTIVE) {
                    detail.status = Some(IMPORT_TASK_STATE_COMPLETED.to_string());
                    // The import is over, so there is no stage left to report and no
                    // percentage left to make.
                    detail.status_message = None;
                    detail.progress = None;
                }
            }
            // The move is applied once; nothing is owed on this task any more.
            self.import_snapshot_conversions.remove(&import_task_id);
        }
    }

    /// The EBS snapshot a converted import snapshot task made, as [`Self::insert_snapshot`]
    /// takes it.
    ///
    /// Everything comes off the task's own detail: the disk image's description, whether the
    /// import was asked for an encrypted snapshot and under which KMS key, and a volume as
    /// large as the disk that was read ("The size of the volume, in GiB",
    /// `Snapshot$VolumeSize`, against "The size of the disk in the snapshot, in GiB",
    /// `SnapshotTaskDetail$DiskImageSize`). The task's own tags are NOT copied onto it —
    /// "the tags to apply to the import snapshot task during creation"
    /// (`ImportSnapshotRequest$TagSpecifications`) tags the task, not the snapshot — which is
    /// the rule [`Self::imported_image_spec`] holds for the AMI its import creates.
    fn imported_snapshot_view(&self, import_task_id: &str) -> Option<SnapshotView> {
        let detail = self
            .import_snapshot_tasks
            .get(import_task_id)?
            .snapshot_task_detail
            .as_ref()?;
        Some(SnapshotView {
            description: detail.description.clone(),
            encrypted: detail.encrypted,
            kms_key_id: detail.kms_key_id.clone(),
            volume_size: detail.disk_image_size.and_then(imported_disk_volume_size),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            storage_tier: Some(SNAPSHOT_STORAGE_TIER_STANDARD.to_string()),
            // Every other member is one an imported snapshot carries nothing for, for the
            // reasons `CreateSnapshot` records against the same shape — and the volume this
            // snapshot was made from is a disk image outside this emulator, so there is no
            // volume of the account to name as its source. The id, the start time, and the
            // state, progress and completion time the clock decides are minted, stamped and
            // derived by the store.
            ..SnapshotView::default()
        })
    }
}

// ---------------------------------------------------------------------------
// Cancelling an import task, whichever resource it imports
// ---------------------------------------------------------------------------
//
// "Cancels an in-process import virtual machine or import snapshot task"
// (`CancelImportTask`): ONE operation acts on a task of either kind, so the state it reads
// and the state it moves live here once rather than once per store. An import image task
// keeps its state on `ImportImageTask$Status` and an import snapshot task on its
// `SnapshotTaskDetail$Status` — the only places either describe reads it from — and both
// move `active` -> `deleting` when a cancel is admitted, then `deleting` -> `deleted` when
// the clock has given that move its span ([`TRANSITION_DELAY`]), so a caller reading
// straight after the cancel still sees the task working through it.

/// How long a cancel reason this store keeps on a task may be.
///
/// CHOSEN, like [`MAX_ECHOED_PAGE_TOKEN_CHARS`]: `CancelImportTaskRequest$CancelReason`
/// publishes no `@length`, and the text is retained on the stored task for the process's
/// lifetime, so a reason past the width EC2 gives its other free-text members is refused
/// rather than stored.
///
/// ONE bound for every reason a cancel of a VM import task keeps, whichever member carries
/// it: `CancelConversionRequest$ReasonMessage` is the same unbounded free text on the same
/// kind of stored record, so it is held to the same width rather than to a second number.
pub const MAX_IMPORT_TASK_CANCEL_REASON_CHARS: usize = 255;

/// Refuse a request naming a CONVERSION task id EC2 could have issued that this account
/// holds no conversion task for: HTTP 400 [`INVALID_CONVERSION_TASK_ID`], EC2's code for a
/// conversion (import) task id it cannot resolve.
///
/// The counterpart of [`invalid_conversion_task_id_format`], which answers the same code
/// for a value that is not a conversion task id at all: an operation acting on ONE named
/// task cannot act on a task that is not there.
///
/// ONE wording for every conversion task of
/// [`ComAmazonawsEc2Amazonec2State::conversion_tasks`] a request names and this account
/// does not hold, whichever operation names it. The IMPORT image and snapshot tasks are
/// answered through [`import_task_id_not_found`] instead, which names the kind of task the
/// id belongs to, the way the service words that refusal for them.
pub fn invalid_conversion_task_id_not_found<E>(import_task_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_CONVERSION_TASK_ID,
        "conversion task",
        import_task_id,
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// The state the import task `import_task_id` reports, or `None` when this account
    /// holds no import task under that id — of either kind, because one operation cancels
    /// either.
    ///
    /// An image task's state is its own `ImportImageTask$Status`; a snapshot task's is its
    /// `SnapshotTaskDetail$Status`. A stored task carrying no status at all reports none
    /// (the empty string), which is no state of [`IMPORT_TASK_STATES`] and so admits no
    /// cancel — every writer of a task sets one.
    pub fn import_task_state(&self, import_task_id: &str) -> Option<String> {
        if let Some(record) = self.import_image_tasks.get(import_task_id) {
            return Some(record.view.status.clone().unwrap_or_default());
        }
        let task = self.import_snapshot_tasks.get(import_task_id)?;
        Some(
            task.snapshot_task_detail
                .as_ref()
                .and_then(|detail| detail.status.clone())
                .unwrap_or_default(),
        )
    }

    /// Start the cancellation of the import task `import_task_id`, which does nothing at
    /// all when this account holds no task under that id.
    ///
    /// The task moves to [`IMPORT_TASK_STATE_DELETING`] now and the clock owes it
    /// [`IMPORT_TASK_STATE_DELETED`] one [`TRANSITION_DELAY`] later
    /// ([`Self::delete_due_cancelled_import_tasks`]). A task that is not converting any
    /// more has no percentage left to make, so no progress is reported for it, and `reason`
    /// — "the reason for canceling the task" (`CancelImportTaskRequest$CancelReason`) — is
    /// written onto the task's status message, the one member either shape carries for a
    /// descriptive status (`ImportImageTask$StatusMessage`,
    /// `SnapshotTaskDetail$StatusMessage`), so the describes report why the task ended
    /// instead of dropping it. Its bytes are caller-chosen text on a stored record, so they
    /// are charged against the store's byte budget the way a tag write is; the message it
    /// replaces is given no refund, because neither writer of a task charges one — an
    /// import image task's is a constant of this module and
    /// [`import_snapshot_task_payload_bytes`] counts the detail's text but not its status
    /// message.
    pub fn start_import_task_cancellation(
        &mut self,
        import_task_id: &str,
        now: Instant,
        reason: Option<&str>,
    ) -> bool {
        let deleting = IMPORT_TASK_STATE_DELETING.to_string();
        let message = reason.map(str::to_string);
        let held = if let Some(record) = self.import_image_tasks.get_mut(import_task_id) {
            record.view.status = Some(deleting.clone());
            record.view.status_message = message.clone();
            record.view.progress = None;
            // Each disk of the import stops where the task did: its own status is the
            // task's ("a brief status for the import image task"), and it has no progress
            // left either.
            for detail in record.view.snapshot_details.iter_mut().flatten() {
                detail.status = Some(deleting.clone());
                detail.progress = None;
            }
            true
        } else if let Some(detail) = self
            .import_snapshot_tasks
            .get_mut(import_task_id)
            .and_then(|task| task.snapshot_task_detail.as_mut())
        {
            detail.status = Some(deleting);
            detail.status_message = message.clone();
            detail.progress = None;
            true
        } else {
            false
        };
        if !held {
            return false;
        }
        self.add_stored_bytes(message.map_or(0, |text| text.len()));
        self.import_task_deleted_at
            .insert(import_task_id.to_string(), now + TRANSITION_DELAY);
        true
    }

    /// Finish every import task cancellation the clock owed by `now`: a task a cancel left
    /// [`IMPORT_TASK_STATE_DELETING`] is [`IMPORT_TASK_STATE_DELETED`] once its instant has
    /// passed.
    ///
    /// Reached only through [`Self::materialize_due_transitions`], so a reporting operation
    /// observes the move without having caused it: it was owed at this instant whether or
    /// not anybody called. The record itself stays in the store — a state is only
    /// reportable while the task it belongs to is there.
    pub fn delete_due_cancelled_import_tasks(&mut self, now: Instant) {
        let image_tasks = &mut self.import_image_tasks;
        let snapshot_tasks = &mut self.import_snapshot_tasks;
        self.import_task_deleted_at.retain(|import_task_id, due_at| {
            if now < *due_at {
                return true;
            }
            if let Some(record) = image_tasks.get_mut(import_task_id) {
                record.view.status = Some(IMPORT_TASK_STATE_DELETED.to_string());
                for detail in record.view.snapshot_details.iter_mut().flatten() {
                    detail.status = Some(IMPORT_TASK_STATE_DELETED.to_string());
                }
            } else if let Some(detail) = snapshot_tasks
                .get_mut(import_task_id)
                .and_then(|task| task.snapshot_task_detail.as_mut())
            {
                detail.status = Some(IMPORT_TASK_STATE_DELETED.to_string());
            }
            // The move is applied once; nothing is owed on this task any more.
            false
        });
    }
}
