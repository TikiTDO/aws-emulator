// SPDX-License-Identifier: CC0-1.0

//! Which NETWORK TYPES a placement supports, read off the addressing of the subnets it is
//! placed in.
//!
//! `Subnet$SupportedNetworkTypes` and `CacheSubnetGroup$SupportedNetworkTypes` are both
//! "Either `ipv4` | `ipv6` | `dual_stack`" (their own documentation) — values of the
//! modelled `NetworkType` enum, which declares them in that order. Which of them a subnet
//! offers follows from the address families the subnet is ADDRESSED in: a subnet with an
//! IPv4 block can carry an IPv4 cache, one with an IPv6 block an IPv6 cache, and one with
//! both can carry either or a dual-stack cache. Those blocks belong to Amazon EC2's record
//! of the subnet, so they are read off what EC2 answered with (see [`super::ec2_subnets`])
//! and nothing is chosen here.
//!
//! A cache is placed ACROSS the subnets of its cache subnet group, so the group offers a
//! network type only when every one of its subnets offers it.

use super::{AccountSubnet, SubnetRecord};

/// `NetworkType` `ipv4`, the type a subnet addressed in IPv4 offers (model enum
/// `NetworkType`: `ipv4 | ipv6 | dual_stack`).
pub const NETWORK_TYPE_IPV4: &str = "ipv4";

/// `NetworkType` `ipv6`; see [`NETWORK_TYPE_IPV4`].
pub const NETWORK_TYPE_IPV6: &str = "ipv6";

/// `NetworkType` `dual_stack`, the type a subnet addressed in BOTH families offers; see
/// [`NETWORK_TYPE_IPV4`].
pub const NETWORK_TYPE_DUAL_STACK: &str = "dual_stack";

/// The network types a subnet addressed in these families supports, in the order the
/// modelled `NetworkType` enum declares them.
///
/// A subnet EC2 reports no block of either family for supports none: there is no address a
/// cache in it could be reached at, and a type is not invented for it.
pub fn subnet_network_types(ipv4: bool, ipv6: bool) -> Vec<String> {
    let mut supported = Vec::with_capacity(3);
    if ipv4 {
        supported.push(NETWORK_TYPE_IPV4.to_string());
    }
    if ipv6 {
        supported.push(NETWORK_TYPE_IPV6.to_string());
    }
    if ipv4 && ipv6 {
        supported.push(NETWORK_TYPE_DUAL_STACK.to_string());
    }
    supported
}

impl AccountSubnet {
    /// The network types THIS subnet supports, from the blocks EC2 reports for it.
    pub fn supported_network_types(&self) -> Vec<String> {
        subnet_network_types(self.has_ipv4_cidr, self.has_ipv6_cidr)
    }
}

/// The network types a group holding these subnets supports: the ones EVERY subnet of it
/// supports, in the order the modelled enum declares them.
///
/// The intersection rather than the union, because a cache is placed across the group's
/// subnets: a type one subnet of the group cannot carry is not one the group can offer. A
/// group holding no subnet offers none.
pub fn group_network_types(subnets: &[SubnetRecord]) -> Vec<String> {
    let mut remaining = subnets.iter();
    let Some(first) = remaining.next() else {
        return Vec::new();
    };
    let mut shared = first.supported_network_types.clone();
    for subnet in remaining {
        shared.retain(|supported| subnet.supported_network_types.contains(supported));
    }
    shared
}

/// `supported` as the modelled `NetworkTypeList` member reports it: absent when there is
/// nothing to report, rather than an empty list.
pub fn reported_network_types(supported: Vec<String>) -> Option<Vec<String>> {
    (!supported.is_empty()).then_some(supported)
}
