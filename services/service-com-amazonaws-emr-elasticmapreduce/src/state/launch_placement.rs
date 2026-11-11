// SPDX-License-Identifier: CC0-1.0

//! WHERE a cluster runs: the Availability Zone its launch placed it in, resolved in the
//! service that owns subnets.
//!
//! One concern of this service's state. `Ec2InstanceAttributes$Ec2AvailabilityZone` is "the
//! Availability Zone in which the cluster will run", and a launch that states its placement
//! as a SUBNET (`JobFlowInstancesConfig$Ec2SubnetId`, "the identifier of the Amazon VPC
//! subnet where you want the cluster to launch") has named that zone indirectly: a subnet
//! exists in exactly one Availability Zone, so the zone the cluster runs in is the zone of
//! the subnet its instances were launched into. Which subnet THAT is, out of what the launch
//! listed, is the sibling reading ([`launched_in`]).
//!
//! The subnet is not this service's resource. It lives in Amazon EC2 and is read THERE,
//! over the in-process callback ([`smithy_server_runtime::OperationContext::callback`]), so
//! this crate names no other service's crate (the same grounding as
//! [`super::studio_network`]). The zone is resolved ONCE, when the cluster is launched, and
//! kept on its record: a cluster does not move, so every later read reports the same zone
//! without asking again.
//!
//! An emulator with no Amazon EC2 mounted, and a subnet Amazon EC2 reports no zone for, know
//! nothing about where the cluster runs — so the cluster reports no Availability Zone rather
//! than one invented for it.

use super::*;

/// The Availability Zone the cluster placed in `subnet_id` runs in, or `None` when Amazon
/// EC2 reports no zone for it (and when the hop could not be made at all, for the reason
/// this module's docs give).
pub async fn subnet_availability_zone(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_id: &str,
) -> Option<String> {
    let answer = studio_network::ec2_client(ctx, region)
        .describe_subnets()
        .subnet_ids(subnet_id)
        .send()
        .await;
    let Ok(output) = answer else {
        return None;
    };
    output
        .subnets()
        .iter()
        .find(|subnet| subnet.subnet_id() == Some(subnet_id))
        .and_then(|subnet| subnet.availability_zone())
        .map(str::to_string)
}

/// The Availability Zone a cluster launched with `instances` runs in.
///
/// A launch states its placement in one of the forms `JobFlowInstancesConfig` takes, and the
/// record already holds the ONE placement out of each: the zone the launch named outright
/// ([`InstancesRecord::ec2_availability_zone`], from `Placement`) is the zone, since the
/// request said so, and a launch that named a SUBNET instead has the subnet's zone resolved
/// in Amazon EC2 ([`subnet_availability_zone`]). A launch that named neither names no
/// placement, and nothing is reported for it.
pub async fn launched_availability_zone(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    instances: &InstancesRecord,
) -> Option<String> {
    if let Some(named) = instances.ec2_availability_zone.clone() {
        return Some(named);
    }
    let subnet_id = instances.ec2_subnet_id.as_deref()?;
    subnet_availability_zone(ctx, region, subnet_id).await
}
