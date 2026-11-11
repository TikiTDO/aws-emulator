// SPDX-License-Identifier: CC0-1.0

//! The private IPv4 addresses ONE network interface specification asks for.
//!
//! The same members, carrying the same documented combination rules, reach this store from
//! two places: `CreateNetworkInterface`'s own request, and each entry of a launch's
//! `NetworkInterfaces` (`RunInstancesRequest$NetworkInterfaces`, whose entries are
//! `InstanceNetworkInterfaceSpecification`). The reading of them lives here, once, so an
//! interface a create stores and one a launch stores cannot be given their addresses on
//! different terms — nor refuse the same combination with two sentences.
//!
//! What this answers is only WHICH addresses were asked for and HOW MANY the interface
//! holds. Taking them is the store's own ([`ComAmazonawsEc2Amazonec2State::reserve_private_ip`]
//! for an address the caller named and
//! [`ComAmazonawsEc2Amazonec2State::mint_private_ip_in_cidr`] for one the service chooses),
//! so every address of a subnet still comes out of the one pool.

use crate::ops::types::PrivateIpAddressSpecification;
use crate::scaffolded::Scaffolded;

use super::{
    ComAmazonawsEc2Amazonec2State, MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE,
    invalid_parameter_combination, invalid_parameter_value, missing_parameter,
};

/// What one interface's private IPv4 address members resolved to.
#[derive(Debug, Clone, Default)]
pub struct RequestedPrivateIps {
    /// The addresses the request NAMED, PRIMARY FIRST: "only one private IPv4 address can be
    /// designated as primary" (`PrivateIpAddressSpecification$Primary`), and the interface
    /// store records the first address of a list as that primary one.
    pub named: Vec<String>,
    /// How many private IPv4 addresses the interface holds in all — the named ones (at least
    /// the primary one every interface that carries addresses has) plus the secondary count
    /// the request asked the service to choose. Zero for an interface that carries none.
    pub wanted: usize,
}

/// Read the private IPv4 address members of ONE interface specification.
///
/// `private_ip_address` is the specification's `PrivateIpAddress`, `specified` its
/// `PrivateIpAddresses`, `secondary_address_count` its `SecondaryPrivateIpAddressCount` and
/// `efa_only` whether its interface type is the one that carries no addresses at all.
///
/// Every refusal here is one of the members' own documented rules, asked BEFORE any address
/// is resolved so the answer never depends on which member was looked at first:
///
/// * "If you specify efa-only, do not assign any IP addresses to the network interface"
///   (`InterfaceType`);
/// * "You can't specify this option and specify more than one private IP address using
///   privateIpAddresses" (`SecondaryPrivateIpAddressCount`);
/// * "Only one IPv4 address can be designated as primary"
///   (`PrivateIpAddressSpecification$Primary`);
/// * "If you specify an IP address, you cannot indicate any IP addresses specified in
///   privateIpAddresses as primary" (`PrivateIpAddress`);
/// * the per-interface ceiling [`MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE`], asked both of the
///   list a caller sent (before any work proportional to its size is done) and of the total
///   the interface would then hold.
pub fn requested_private_ips<E>(
    private_ip_address: Option<&str>,
    specified: &[PrivateIpAddressSpecification],
    secondary_address_count: i32,
    efa_only: bool,
) -> Result<RequestedPrivateIps, Scaffolded<E>> {
    // `PrivateIpAddresses` carries no `@length`, so the list a caller may send is unbounded;
    // the per-interface ceiling is enforced HERE, before any work proportional to the list's
    // size — parsing an address, probing the in-use set — is done.
    if specified.len() > MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE {
        return Err(invalid_parameter_value(format!(
            "The request names {} private IPv4 addresses, above the maximum of {max} \
             per network interface",
            specified.len(),
            max = MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE,
        )));
    }
    if secondary_address_count < 0 {
        return Err(invalid_parameter_value(format!(
            "Invalid value '{secondary_address_count}' for secondaryPrivateIpAddressCount. \
             It cannot be negative"
        )));
    }
    if efa_only
        && (private_ip_address.is_some() || !specified.is_empty() || secondary_address_count > 0)
    {
        return Err(invalid_parameter_combination(
            "An efa-only network interface does not support IP addresses",
        ));
    }
    if secondary_address_count > 0 && specified.len() > 1 {
        return Err(invalid_parameter_combination(
            "You cannot specify SecondaryPrivateIpAddressCount and more than one \
             address in PrivateIpAddresses",
        ));
    }
    if specified
        .iter()
        .filter(|address| address.primary.unwrap_or(false))
        .count()
        > 1
    {
        return Err(invalid_parameter_value(
            "Only one IPv4 address can be designated as primary",
        ));
    }
    if private_ip_address.is_some()
        && specified
            .iter()
            .any(|address| address.primary.unwrap_or(false))
    {
        return Err(invalid_parameter_combination(
            "You cannot designate an address in PrivateIpAddresses as primary when \
             PrivateIpAddress is specified",
        ));
    }
    // Each entry has to carry the address it specifies: an entry with none names nothing to
    // assign.
    for address in specified {
        if address
            .private_ip_address
            .as_deref()
            .is_none_or(str::is_empty)
        {
            return Err(missing_parameter(
                "Each entry of PrivateIpAddresses must carry a privateIpAddress",
            ));
        }
    }

    // The primary address the request named, if it named one: `PrivateIpAddress` itself, or
    // the entry of `PrivateIpAddresses` marked primary. It leads the list, which is where the
    // interface store reads the primary address of an interface from.
    let primary = private_ip_address.map(str::to_string).or_else(|| {
        specified
            .iter()
            .find(|address| address.primary.unwrap_or(false))
            .and_then(|address| address.private_ip_address.clone())
    });
    let mut named: Vec<String> = primary.clone().into_iter().collect();
    named.extend(
        specified
            .iter()
            .filter_map(|address| address.private_ip_address.clone())
            .filter(|address| primary.as_deref() != Some(address.as_str())),
    );
    // "When you specify a number of secondary IPv4 addresses, Amazon EC2 selects these IP
    // addresses within the subnet's IPv4 CIDR range" (`SecondaryPrivateIpAddressCount`): they
    // are additional to the primary address, which every interface that carries addresses at
    // all has — so an interface asks for the addresses the request named (at least the
    // primary one) plus that count.
    let wanted = if efa_only {
        0
    } else {
        named
            .len()
            .max(1)
            .saturating_add(usize::try_from(secondary_address_count).unwrap_or_default())
    };
    if wanted > MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE {
        return Err(invalid_parameter_value(format!(
            "The request asks for {wanted} private IPv4 addresses, above the maximum \
             of {max} per network interface",
            max = MAX_PRIVATE_IPS_PER_NETWORK_INTERFACE,
        )));
    }
    Ok(RequestedPrivateIps { named, wanted })
}

impl ComAmazonawsEc2Amazonec2State {
    /// Hand back every private IPv4 address a half-finished resolution had already taken.
    ///
    /// The taking of an address is a WRITE ([`Self::reserve_private_ip`] and
    /// [`Self::mint_private_ip_in_cidr`] both record it in the one in-use pool), so a request
    /// that is refused after taking some has to give them back or they are held by nothing
    /// for the lifetime of the store. One definition of that give-back, for every path that
    /// takes addresses for an interface.
    pub fn release_private_ips(&mut self, addresses: &[String]) {
        for address in addresses {
            self.release_private_ip(address);
        }
    }
}
