// SPDX-License-Identifier: CC0-1.0

//! The launch's own instance configuration: what a cluster reports back of the
//! `JobFlowInstancesConfig` it was created with.
//!
//! One concern of this service's state. A launch states its instances and where they go, and
//! [`InstancesRecord`] is what the cluster keeps of that — the types, the counts, the key
//! pair, the security groups, the subnet and zone it runs in, and the two settings that
//! govern its shutdown and its node replacement. The ONE placement out of a LIST of
//! candidates is read here ([`launched_in`]); the zone a launch named only through a subnet
//! is resolved in the service that owns subnets ([`super::launch_placement`]).

/// The `JobFlowInstancesConfig`-derived facts a cluster reports back.
#[derive(Debug, Clone, Default)]
pub struct InstancesRecord {
    pub instance_collection_type: String,
    pub master_instance_type: Option<String>,
    pub slave_instance_type: Option<String>,
    pub instance_count: Option<i32>,
    pub ec2_key_name: Option<String>,
    pub ec2_subnet_id: Option<String>,
    pub requested_ec2_subnet_ids: Vec<String>,
    pub ec2_availability_zone: Option<String>,
    pub requested_ec2_availability_zones: Vec<String>,
    pub emr_managed_master_security_group: Option<String>,
    pub emr_managed_slave_security_group: Option<String>,
    pub service_access_security_group: Option<String>,
    pub additional_master_security_groups: Vec<String>,
    pub additional_slave_security_groups: Vec<String>,
    pub keep_job_flow_alive_when_no_steps: bool,
    pub termination_protected: bool,
    pub unhealthy_node_replacement: bool,
}

/// The ONE placement a cluster's instances were launched in, out of what its
/// `JobFlowInstancesConfig` asked for.
///
/// A cluster states its placement either as a single value (`Ec2SubnetId`,
/// `Placement$AvailabilityZone`) or — the instance-fleets form — as a LIST of candidates
/// (`Ec2SubnetIds`, `Placement$AvailabilityZones`). A list is a request for a choice, not a
/// placement: "Amazon EMR chooses the Amazon EC2 subnet with the best fit from among the
/// list of `RequestedEc2SubnetIds`, and then launches all cluster instances within that
/// Subnet" (`Ec2InstanceAttributes$RequestedEc2SubnetIds`), and the same for the zones
/// ("chooses the Availability Zone with the best fit from among the list of
/// `RequestedEc2AvailabilityZones`"). So a cluster configured with a candidate list is
/// still running in exactly one subnet and one zone, and the singular member reports that
/// one while the plural member keeps the candidates the caller named.
///
/// There is no capacity here to rank candidates by fit, so the choice is the FIRST
/// candidate the request lists: a value the caller supplied, decided once when the cluster
/// is created and recorded, so every later read reports the same placement. A single value
/// already IS the placement and wins over the list, and a cluster that named neither is
/// reported with no placement rather than one invented for it.
pub(crate) fn launched_in(single: Option<&str>, candidates: &[String]) -> Option<String> {
    single
        .map(str::to_string)
        .or_else(|| candidates.first().cloned())
}
