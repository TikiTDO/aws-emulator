// SPDX-License-Identifier: CC0-1.0

//! Whether the security groups of a registered target's own network interface let a load
//! balancer's HEALTH CHECK arrive at all.
//!
//! A health check is an ordinary TCP connection the load balancer opens to the target on the
//! group's effective health-check port ([`super::TargetGroupRecord::health_check_port_of`]),
//! so it is governed by the same inbound rules as any other traffic to that interface: a
//! security group "controls the traffic that is allowed to reach and leave the resources that
//! it is associated with", and its inbound rules "control the incoming traffic that's allowed
//! to reach the instance" (Amazon VPC User Guide, "Security groups"). A group admits only what
//! a rule of it names, so a target whose groups carry no inbound rule for that port from that
//! load balancer is a target no check can reach — and a check that cannot be delivered is a
//! check that fails, which is what drives the target to `unhealthy` once the group's
//! `UnhealthyThresholdCount` consecutive checks have failed
//! ([`super::target_health::TargetProbe`]).
//!
//! The rules themselves are Amazon EC2's records, read out of that service's own store exactly
//! as every other fact this service does not own is ([`super::network`]). Nothing is assumed
//! from silence: a group the inventory does not hold, a rule naming a prefix list this service
//! cannot resolve, and a load balancer whose own groups and addresses are unknown all leave the
//! question UNDECIDED ([`interface_admits_check`] answers `None`), and an undecided question
//! fails no check.

use ::std::collections::{BTreeSet, HashMap};
use ::std::sync::Arc;

use ::service_com_amazonaws_ec2_amazonec2::ops::types as ec2_types;

/// The security groups GOVERNING one registration's address, as group ids.
///
/// What the scheduler that runs the workload behind a registration declares them to be
/// (`AwsVpcConfiguration$securityGroups` for an Amazon ECS task), and empty for a
/// registration nothing declares them for — whose groups are the ones the network interface
/// holding its address reports instead.
///
/// SHARED, not copied: one workload's groups are the same list for every registration that
/// stands for it, so registering it in several target groups costs one allocation and cloning
/// a registration costs none — the same reason [`super::ServedPorts`] is shared.
pub type GuardingGroups = Arc<[String]>;

/// Where a health check arrives FROM: the load balancer that performs it, as the inbound rules
/// of the target's groups can name it.
///
/// A rule names its source either as another security group — "the source is the security
/// group associated with the load balancer" — or as an address range, so both are carried: the
/// groups the load balancer is associated with, their names (a rule may name a group by name
/// rather than by id), and the addresses its own network interfaces hold
/// ([`super::service_interfaces`]), which are the addresses its checks come from.
///
/// Every field may be empty, which says that this service does not know that half of the
/// source rather than that the load balancer has none.
#[derive(Debug, Default, Clone)]
pub struct CheckSource {
    /// The `GroupId` of each security group the load balancer is associated with.
    pub group_ids: Vec<String>,
    /// The `GroupName` of those same groups, for a rule that names its source group by name.
    pub group_names: Vec<String>,
    /// The addresses the load balancer's own network interfaces hold.
    pub addresses: Vec<String>,
}

impl CheckSource {
    /// Whether this source says nothing at all about where a check comes from, in which case
    /// no rule can be read as naming it or as not naming it.
    fn is_unknown(&self) -> bool {
        self.group_ids.is_empty() && self.group_names.is_empty() && self.addresses.is_empty()
    }
}

/// Whether the security groups `groups` names admit a health check on `port` from `source`:
/// `Some(true)` when an inbound rule of one of them names it, `Some(false)` when every rule
/// of every one of them is known and none does, and `None` when the question is undecided.
///
/// `ingress` is the inbound rules of the account's security groups, keyed by `GroupId`. A
/// group absent from it is a group whose rules are unknown, so the whole question is: a
/// verdict is never reached on rules this service could not read.
pub fn interface_admits_check(
    groups: &[String],
    ingress: &HashMap<String, Vec<ec2_types::IpPermission>>,
    port: i32,
    source: &CheckSource,
) -> Option<bool> {
    if groups.is_empty() || source.is_unknown() {
        return None;
    }
    // Each group is read ONCE, however often it is named: a group named twice governs nothing
    // twice, so the work this costs is the number of DISTINCT groups times their rules rather
    // than the length of the list that named them.
    let distinct: BTreeSet<&str> = groups.iter().map(String::as_str).collect();
    let mut undecided = false;
    let mut admitted = false;
    for group in distinct {
        // A group whose rules are not held decides nothing on its own, and leaves nothing
        // decidable by the others: one of its rules could admit the check. It does not undo
        // a rule that already admitted it, though — one group admitting the check is enough.
        let Some(permissions) = ingress.get(group) else {
            undecided = true;
            continue;
        };
        for permission in permissions {
            if !covers_tcp_port(permission, port) {
                continue;
            }
            match admits_source(permission, source) {
                Some(true) => admitted = true,
                Some(false) => {}
                None => undecided = true,
            }
        }
    }
    if admitted {
        return Some(true);
    }
    if undecided {
        return None;
    }
    Some(false)
}

/// Whether one inbound rule covers a TCP connection to `port`.
///
/// `IpPermission$IpProtocol` is "the IP protocol name (`tcp`, `udp`, `icmp`, `icmpv6`) or
/// number ... Use `-1` to specify all protocols", and `FromPort`/`ToPort` are "the first port
/// in the range" and "the last port in the range" — so a rule for all protocols covers every
/// port, a TCP rule covers the ports of its own range, and a rule for another protocol covers
/// no TCP connection at all. A TCP rule carrying no range names no bound, so it bounds
/// nothing.
fn covers_tcp_port(permission: &ec2_types::IpPermission, port: i32) -> bool {
    let protocol = permission
        .ip_protocol
        .as_deref()
        .unwrap_or(ALL_PROTOCOLS)
        .trim()
        .to_ascii_lowercase();
    match protocol.as_str() {
        ALL_PROTOCOLS => true,
        TCP_PROTOCOL_NAME | TCP_PROTOCOL_NUMBER => {
            permission.from_port.is_none_or(|from| from <= port)
                && permission.to_port.is_none_or(|to| port <= to)
        }
        _ => false,
    }
}

/// The `IpProtocol` value that names every protocol: "use `-1` to specify all protocols".
const ALL_PROTOCOLS: &str = "-1";

/// The `IpProtocol` name of the protocol a health check connects over.
const TCP_PROTOCOL_NAME: &str = "tcp";

/// The IANA protocol number of that same protocol, which `IpProtocol` may carry instead of its
/// name.
const TCP_PROTOCOL_NUMBER: &str = "6";

/// Whether one rule names `source` as a source it admits: `Some(true)` when one of its grants
/// names it, `Some(false)` when every grant is readable and none does, and `None` when a grant
/// cannot be read against this source.
fn admits_source(permission: &ec2_types::IpPermission, source: &CheckSource) -> Option<bool> {
    let mut undecided = false;
    for pair in permission.user_id_group_pairs.iter().flatten() {
        let names_group = pair
            .group_id
            .as_deref()
            .is_some_and(|id| source.group_ids.iter().any(|held| held == id))
            || pair
                .group_name
                .as_deref()
                .is_some_and(|name| source.group_names.iter().any(|held| held == name));
        if names_group {
            return Some(true);
        }
        if source.group_ids.is_empty() && source.group_names.is_empty() {
            // The groups the load balancer is associated with are not known, so a grant
            // naming a group cannot be read either way.
            undecided = true;
        }
    }
    for range in permission.ip_ranges.iter().flatten() {
        match range
            .cidr_ip
            .as_deref()
            .and_then(|cidr| block_admits(cidr, &source.addresses))
        {
            Some(true) => return Some(true),
            Some(false) => {}
            None => undecided = true,
        }
    }
    for range in permission.ipv6_ranges.iter().flatten() {
        match range
            .cidr_ipv6
            .as_deref()
            .and_then(|cidr| block_admits(cidr, &source.addresses))
        {
            Some(true) => return Some(true),
            Some(false) => {}
            None => undecided = true,
        }
    }
    if permission
        .prefix_list_ids
        .iter()
        .flatten()
        .next()
        .is_some()
    {
        // "The prefix list IDs" name a managed set of ranges this service does not resolve, so
        // a rule carrying one says nothing readable about this source.
        undecided = true;
    }
    if undecided {
        return None;
    }
    Some(false)
}

/// Whether the address range `cidr` admits any of `addresses`: `Some(true)` when it holds one
/// of them, `Some(false)` when it holds none, and `None` when the question cannot be read —
/// a range that is not an address block, or a block narrower than the whole address family
/// with no address of the source to read against it.
///
/// A block whose prefix length is zero is the whole address family, which holds every address
/// of it however little is known about the source.
fn block_admits(cidr: &str, addresses: &[String]) -> Option<bool> {
    let block: ::ipnet::IpNet = cidr.trim().parse().ok()?;
    if block.prefix_len() == 0 {
        return Some(true);
    }
    if addresses.is_empty() {
        return None;
    }
    // The CONTAINMENT is this crate's one reading of it ([`super::network::inside_any`]), so
    // a range read here and a range read anywhere else in the service hold the same
    // addresses.
    Some(addresses.iter().any(|address| {
        address
            .trim()
            .parse::<::std::net::IpAddr>()
            .is_ok_and(|address| {
                super::network::inside_any(::std::slice::from_ref(&block), address)
            })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One inbound rule, as a target's group carries it.
    fn permission(
        protocol: &str,
        ports: Option<(i32, i32)>,
        groups: &[&str],
        cidrs: &[&str],
    ) -> ec2_types::IpPermission {
        ec2_types::IpPermission {
            ip_protocol: Some(protocol.to_string()),
            from_port: ports.map(|(from, _)| from),
            to_port: ports.map(|(_, to)| to),
            user_id_group_pairs: Some(
                groups
                    .iter()
                    .map(|id| ec2_types::UserIdGroupPair {
                        group_id: Some((*id).to_string()),
                        ..ec2_types::UserIdGroupPair::default()
                    })
                    .collect(),
            ),
            ip_ranges: Some(
                cidrs
                    .iter()
                    .map(|cidr| ec2_types::IpRange {
                        cidr_ip: Some((*cidr).to_string()),
                        ..ec2_types::IpRange::default()
                    })
                    .collect(),
            ),
            ipv6_ranges: None,
            prefix_list_ids: None,
        }
    }

    /// The load balancer every case below checks from.
    fn source() -> CheckSource {
        CheckSource {
            group_ids: vec!["sg-0000000000000000a".to_string()],
            group_names: vec!["alb".to_string()],
            addresses: vec!["10.0.0.5".to_string()],
        }
    }

    /// RULE: a check is admitted only by an inbound rule that covers its port AND names the
    /// load balancer performing it; every rule of every group of the interface is read, and a
    /// rule this service cannot read leaves the question undecided.
    #[test]
    fn a_check_is_admitted_only_by_a_rule_that_names_its_port_and_its_source() {
        let target_group = "sg-0000000000000000b".to_string();
        for (case, rules, expected) in [
            ("no inbound rule at all", vec![], Some(false)),
            (
                "the load balancer's group on the checked port",
                vec![permission("tcp", Some((80, 80)), &["sg-0000000000000000a"], &[])],
                Some(true),
            ),
            (
                "the load balancer's group on another port",
                vec![permission("tcp", Some((8080, 8080)), &["sg-0000000000000000a"], &[])],
                Some(false),
            ),
            (
                "another group on the checked port",
                vec![permission("tcp", Some((80, 80)), &["sg-0000000000000000c"], &[])],
                Some(false),
            ),
            (
                "a port range that spans the checked port",
                vec![permission("tcp", Some((70, 90)), &["sg-0000000000000000a"], &[])],
                Some(true),
            ),
            (
                "every protocol, so every port",
                vec![permission("-1", None, &["sg-0000000000000000a"], &[])],
                Some(true),
            ),
            (
                "the protocol number of the one a check connects over",
                vec![permission("6", Some((80, 80)), &["sg-0000000000000000a"], &[])],
                Some(true),
            ),
            (
                "another protocol on the checked port",
                vec![permission("udp", Some((80, 80)), &["sg-0000000000000000a"], &[])],
                Some(false),
            ),
            (
                "an address range holding the load balancer's own address",
                vec![permission("tcp", Some((80, 80)), &[], &["10.0.0.0/16"])],
                Some(true),
            ),
            (
                "an address range holding no address of it",
                vec![permission("tcp", Some((80, 80)), &[], &["192.168.0.0/16"])],
                Some(false),
            ),
            (
                "the whole address family",
                vec![permission("tcp", Some((80, 80)), &[], &["0.0.0.0/0"])],
                Some(true),
            ),
            (
                "a range that is no address block at all",
                vec![permission("tcp", Some((80, 80)), &[], &["not-a-block"])],
                None,
            ),
            (
                "one rule for another group and one for the right one",
                vec![
                    permission("tcp", Some((80, 80)), &["sg-0000000000000000c"], &[]),
                    permission("tcp", Some((80, 80)), &["sg-0000000000000000a"], &[]),
                ],
                Some(true),
            ),
        ] {
            let ingress = HashMap::from([(target_group.clone(), rules)]);
            let admitted = interface_admits_check(
                ::std::slice::from_ref(&target_group),
                &ingress,
                80,
                &source(),
            );
            assert_eq!(
                admitted, expected,
                "case {case:?}: expected {expected:?}, got {admitted:?}"
            );
            // A group named twice governs nothing twice, so repeating it decides the same.
            let repeated = vec![target_group.clone(), target_group.clone()];
            let again = interface_admits_check(&repeated, &ingress, 80, &source());
            assert_eq!(
                again, expected,
                "case {case:?} with the group named twice: expected {expected:?}, got {again:?}"
            );
            // A second group whose rules are not held leaves the question undecided where
            // the first group did not already admit the check, and cannot UNDO an admission.
            let beside_unknown = vec![target_group.clone(), "sg-0000000000000000f".to_string()];
            let with_unknown = interface_admits_check(&beside_unknown, &ingress, 80, &source());
            let expected_with_unknown = (expected == Some(true)).then_some(true);
            assert_eq!(
                with_unknown, expected_with_unknown,
                "case {case:?} beside a group whose rules are not held: expected \
                 {expected_with_unknown:?}, got {with_unknown:?}"
            );
        }
    }

    /// Nothing is assumed from silence: an interface with no group, a group whose rules the
    /// inventory does not hold, and a load balancer nothing is known about all leave the
    /// question undecided.
    #[test]
    fn an_unknown_rule_or_an_unknown_source_decides_nothing() {
        let group = "sg-0000000000000000b".to_string();
        let held = HashMap::from([(
            group.clone(),
            vec![permission("tcp", Some((80, 80)), &["sg-0000000000000000a"], &[])],
        )]);
        for (case, groups, ingress, source) in [
            (
                "an interface in no security group",
                Vec::new(),
                held.clone(),
                source(),
            ),
            (
                "a group whose rules are not held",
                vec!["sg-0000000000000000d".to_string()],
                held.clone(),
                source(),
            ),
            (
                "a load balancer nothing is known about",
                vec![group.clone()],
                held.clone(),
                CheckSource::default(),
            ),
            (
                "a rule naming a prefix list",
                vec![group.clone()],
                HashMap::from([(
                    group.clone(),
                    vec![ec2_types::IpPermission {
                        ip_protocol: Some("tcp".to_string()),
                        from_port: Some(80),
                        to_port: Some(80),
                        prefix_list_ids: Some(vec![ec2_types::PrefixListId {
                            prefix_list_id: Some("pl-0123456789abcdef0".to_string()),
                            ..ec2_types::PrefixListId::default()
                        }]),
                        ..ec2_types::IpPermission::default()
                    }],
                )]),
                source(),
            ),
            (
                "a narrow range with no address of the load balancer to read against it",
                vec![group.clone()],
                HashMap::from([(
                    group.clone(),
                    vec![permission("tcp", Some((80, 80)), &[], &["10.0.0.0/16"])],
                )]),
                CheckSource {
                    group_ids: vec!["sg-0000000000000000e".to_string()],
                    ..CheckSource::default()
                },
            ),
        ] {
            let admitted = interface_admits_check(&groups, &ingress, 80, &source);
            assert_eq!(
                admitted, None,
                "case {case:?}: expected None, got {admitted:?}"
            );
        }
    }
}
