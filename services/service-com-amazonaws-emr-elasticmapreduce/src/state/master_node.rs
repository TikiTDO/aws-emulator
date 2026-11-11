// SPDX-License-Identifier: CC0-1.0

//! A cluster's master node: the address Amazon EMR gives it and the DNS name the cluster
//! reports for it.
//!
//! `Cluster$MasterPublicDnsName` is the DNS name of the master node — the private DNS
//! name while the cluster runs on a private subnet, the public one while it runs on a
//! public subnet. A master node is one Amazon EC2 instance, and nothing here gives that
//! instance a PUBLIC address: no address of this emulator is reachable from outside the
//! process, so none is reported as one. What a cluster reports is therefore the
//! Amazon-provided PRIVATE hostname of the address its master node holds, which is the
//! name AWS reports for a master node that holds no public address.
//!
//! The address is decided when the cluster is LAUNCHED and kept on its record
//! ([`super::ClusterRecord`]), so every later read reports the same name and no two
//! clusters of the account hold the same address. WHEN a cluster reports it is
//! [`super::ClusterRecord::master_public_dns_name`]: only once the clock has brought the
//! master node up.

/// How many host addresses one `10.0.<subnet>.0/24` of the master network offers: the
/// first and the last address of an IPv4 subnet are its network and broadcast addresses,
/// so neither is given to a node.
const HOSTS_PER_SUBNET: u64 = 254;

/// The one Region whose Amazon-provided private hostnames carry the short private domain
/// rather than one built from the Region name.
const EC2_INTERNAL_REGION: &str = "us-east-1";

/// The private IPv4 address the master node of the `sequence`-th cluster of this account
/// holds.
///
/// The addresses are laid out over `10.0.0.0/16` — one of the ranges RFC 1918 reserves
/// for private use, which is what an Amazon EC2 instance holds a private address out of —
/// one host address per cluster in launch order, skipping each subnet's network and
/// broadcast address.
fn master_private_ip_address(sequence: u64) -> String {
    let host = sequence.saturating_sub(1);
    let subnet = (host / HOSTS_PER_SUBNET) % 256;
    let address = host % HOSTS_PER_SUBNET + 1;
    format!("10.0.{subnet}.{address}")
}

/// The private DNS domain of `region`: the suffix every Amazon-provided private hostname
/// in it carries.
///
/// AWS uses `ec2.internal` in `us-east-1` and `<region>.compute.internal` in every other
/// Region (Amazon VPC User Guide, "DHCP option sets").
fn private_dns_domain(region: &str) -> String {
    if region == EC2_INTERNAL_REGION {
        "ec2.internal".to_string()
    } else {
        format!("{region}.compute.internal")
    }
}

/// The DNS name the master node of the `sequence`-th cluster of this account carries in
/// `region`.
///
/// AWS builds the `ip-name` style hostname as `ip-` followed by the node's private
/// address with its dots replaced by dashes, then the Region's private DNS domain
/// ([`private_dns_domain`]) — Amazon EC2 User Guide, "Amazon EC2 instance hostname
/// types".
pub fn master_dns_name(region: &str, sequence: u64) -> String {
    let host = master_private_ip_address(sequence).replace('.', "-");
    format!("ip-{host}.{domain}", domain = private_dns_domain(region))
}

/// The host this emulator ANSWERS on for the request being served, or `None` when it answers
/// on none.
///
/// A cluster's `MasterPublicDnsName` is an address a client CONNECTS to, so where this
/// emulator is served on a host, that host is the one it can be reached at and the one a
/// cluster names its master node by ([`super::ClusterRecord::master_public_dns_name`]). The
/// reading is the tree's one reading of that question
/// ([`::service_common::data_plane_addressing`]), in the order every announced address here
/// resolves it: the emulator's OWN endpoint when the request addressed it — a client that
/// resolves nothing else — and otherwise the address this process is served at, which is
/// what an in-process call of another service is told. The PORT is dropped, because what
/// this names is a host.
pub fn served_master_host(ctx: &::smithy_server_runtime::OperationContext<'_>) -> Option<String> {
    let parts = ctx.parts();
    let header = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|host| host.to_str().ok());
    let authority = ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        header,
    )
    .or_else(|| {
        ::service_common::data_plane_addressing::served_endpoint_authority(ctx.endpoint().as_str())
    })?;
    Some(
        ::service_common::data_plane_addressing::host_only(&authority).to_string(),
    )
}
