// SPDX-License-Identifier: CC0-1.0

//! The NETWORK TYPES a cache subnet group and its subnets publish, and the refusal a
//! `SubnetIds` list spanning two VPCs gets.
//!
//! Two rules, each over a SET of inputs rather than one recorded request:
//!
//! 1. `Subnet$SupportedNetworkTypes` and `CacheSubnetGroup$SupportedNetworkTypes` are both
//!    "Either `ipv4` | `ipv6` | `dual_stack`" (their own documentation), and which of them
//!    a subnet offers follows from the address families it is ADDRESSED in, as Amazon EC2 —
//!    the service that owns subnets — reports them: an IPv4 block means `ipv4`, an IPv6
//!    block means `ipv6`, and a subnet carrying both also offers `dual_stack`. A group is
//!    the set of subnets a cache is placed across, so it offers a type only when every one
//!    of its subnets offers it. The rows differ along what the rule generalises over: the
//!    address families a subnet carries (each of the four combinations), how many subnets a
//!    group holds, whose VPC they sit in, and what the subnets of one group offer each
//!    other.
//!
//! 2. A group holds the subnets of ONE VPC — a cache placed in it sits in one VPC — so a
//!    `SubnetIds` list whose subnets span two names a subnet that does not belong in the
//!    group: the modelled `InvalidSubnet`, worded `Subnets <a> and <b> are not in the same
//!    VPC.` over the two identifiers that disagree, with nothing stored. The rows span both
//!    operations that build a group out of subnet ids, both orders of the two VPCs and two
//!    list lengths.
//!
//! Every wire assertion goes through the same router the production emulator mounts, with
//! EC2 mounted beside ElastiCache on one engine by [`crate::harness`]; the subnets a row
//! names were MINTED by EC2 and what they are addressed for is read back off EC2's own
//! record, so nothing here can pass itself off as a subnet the account holds.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::subnet_group_params;
use crate::harness::{params, Emulator, Reply, REGION};

/// The Availability Zones the client-minted subnets below sit in, written as
/// `<region><letter>` — the form AWS names a zone of a Region in.
const ZONES: [&str; 3] = ["us-east-1a", "us-east-1b", "us-east-1c"];

/// One subnet of a group as the wire reports it, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WireSubnet {
    identifier: String,
    zone: String,
    network_types: Vec<String>,
}

/// Every `Subnet` of the one `CacheSubnetGroup` a reply reports.
///
/// The members of a `Subnet` arrive in the order the shape declares them, so each
/// `SubnetIdentifier` opens the subnet the following members belong to.
fn wire_subnets(case: &str, reply: &Reply) -> Vec<WireSubnet> {
    let mut reported: Vec<WireSubnet> = Vec::new();
    for (path, text) in &reply.nodes {
        if path.ends_with("/Subnets/Subnet/SubnetIdentifier") {
            reported.push(WireSubnet {
                identifier: text.clone(),
                zone: String::new(),
                network_types: Vec::new(),
            });
            continue;
        }
        let Some(subnet) = reported.last_mut() else {
            continue;
        };
        if path.ends_with("/Subnets/Subnet/SubnetAvailabilityZone/Name") {
            subnet.zone = text.clone();
        } else if path.ends_with("/Subnets/Subnet/SupportedNetworkTypes/member") {
            subnet.network_types.push(text.clone());
        }
    }
    assert!(
        !reported.is_empty(),
        "case {case}: the group must report the subnets it holds, got {}",
        reply.body
    );
    reported
}

/// The network types the one `CacheSubnetGroup` of a reply publishes for ITSELF.
fn wire_group_network_types(reply: &Reply) -> Vec<String> {
    reply
        .nodes
        .iter()
        .filter(|(path, _)| path.ends_with("/CacheSubnetGroup/SupportedNetworkTypes/member"))
        .map(|(_, text)| text.clone())
        .collect()
}

/// The network types the subnet `subnet_id` offers, derived from the address families EC2
/// reports for it: its IPv4 block, and any IPv6 block associated with it.
///
/// ASKED of the owner, never assumed — and derived here rather than read from the service
/// under test, so a group that published something else would fail.
async fn expected_network_types(emulator: &Emulator, subnet_id: &str) -> Vec<String> {
    let described = emulator
        .ec2(REGION)
        .describe_subnets()
        .subnet_ids(subnet_id)
        .send()
        .await
        .unwrap_or_else(|error| panic!("EC2 must report the subnet {subnet_id}: {error:?}"));
    let subnet = described
        .subnets()
        .iter()
        .find(|subnet| subnet.subnet_id() == Some(subnet_id))
        .unwrap_or_else(|| panic!("EC2 must hold the subnet {subnet_id}"));
    let ipv4 = subnet.cidr_block().is_some_and(|cidr| !cidr.is_empty());
    let ipv6 = subnet
        .ipv6_cidr_block_association_set()
        .iter()
        .any(|association| {
            association
                .ipv6_cidr_block()
                .is_some_and(|cidr| !cidr.is_empty())
        });
    let mut expected: Vec<String> = Vec::new();
    if ipv4 {
        expected.push("ipv4".to_string());
    }
    if ipv6 {
        expected.push("ipv6".to_string());
    }
    if ipv4 && ipv6 {
        expected.push("dual_stack".to_string());
    }
    expected
}

/// The account's network for these cases: the default subnets EC2 reports for [`REGION`],
/// and three a client minted in a VPC of its own.
struct Network {
    emulator: Emulator,
    default_subnets: Vec<String>,
    client_subnets: Vec<String>,
}

async fn network() -> Network {
    let emulator = Emulator::new();
    let ec2 = emulator.ec2(REGION);
    let default_subnets: Vec<String> = emulator
        .account_subnets()
        .await
        .into_iter()
        .filter(|subnet| subnet.default_for_az)
        .map(|subnet| subnet.id)
        .collect();
    let client_vpc = ec2
        .create_vpc()
        .cidr_block("10.7.0.0/16")
        .send()
        .await
        .expect("EC2 must create the client's VPC")
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .expect("CreateVpc must report the VPC it minted")
        .to_string();
    let mut client_subnets = Vec::with_capacity(ZONES.len());
    for (index, zone) in ZONES.iter().enumerate() {
        client_subnets.push(
            ec2.create_subnet()
                .vpc_id(&client_vpc)
                .cidr_block(format!("10.7.{}.0/24", index + 1))
                .availability_zone(*zone)
                .send()
                .await
                .unwrap_or_else(|error| panic!("EC2 must create a subnet in {zone}: {error:?}"))
                .subnet()
                .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
                .unwrap_or_else(|| panic!("CreateSubnet must report the subnet it minted"))
                .to_string(),
        );
    }
    Network {
        emulator,
        default_subnets,
        client_subnets,
    }
}

#[tokio::test]
async fn a_group_and_its_subnets_publish_the_network_types_ec2_addresses_them_for() {
    let net = network().await;
    let defaults = net.default_subnets.clone();
    let client = net.client_subnets.clone();

    // The axis: how many subnets the group holds, whose VPC they sit in, and the order they
    // are named in.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("one default subnet", vec![defaults[0].clone()]),
        (
            "two default subnets",
            vec![defaults[0].clone(), defaults[1].clone()],
        ),
        ("one client-minted subnet", vec![client[0].clone()]),
        (
            "three client-minted subnets",
            vec![client[0].clone(), client[1].clone(), client[2].clone()],
        ),
        (
            "the same three in another order",
            vec![client[2].clone(), client[0].clone(), client[1].clone()],
        ),
    ];

    for (index, (case, ids)) in rows.iter().enumerate() {
        let name = format!("net-types-{index}");
        let created = net
            .emulator
            .call(
                "CreateCacheSubnetGroup",
                &subnet_group_params(&name, case, ids),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: the group must be created, got {} {}",
            created.status, created.body
        );

        // The create's answer and a later read of the same group report the same thing: the
        // group is persisted, not echoed.
        let described = net
            .emulator
            .call(
                "DescribeCacheSubnetGroups",
                &params(&[("CacheSubnetGroupName", &name)]),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case}: the group must be readable back, got {} {}",
            described.status, described.body
        );
        for (reader, reply) in [("create", &created), ("describe", &described)] {
            let subnets = wire_subnets(case, reply);
            assert_eq!(
                subnets
                    .iter()
                    .map(|subnet| subnet.identifier.clone())
                    .collect::<Vec<String>>(),
                *ids,
                "case {case} / {reader}: the group must hold the subnets the request named, \
                 got {subnets:?}"
            );
            for subnet in &subnets {
                let expected = expected_network_types(&net.emulator, &subnet.identifier).await;
                assert_eq!(
                    subnet.network_types, expected,
                    "case {case} / {reader}: {} must offer the types EC2 addresses it for, \
                     got {:?}",
                    subnet.identifier, subnet.network_types
                );
                assert!(
                    !subnet.zone.is_empty(),
                    "case {case} / {reader}: {} must report its zone, got {:?}",
                    subnet.identifier,
                    subnet.zone
                );
            }
            // The group offers what EVERY one of its subnets offers.
            let mut shared = subnets[0].network_types.clone();
            for subnet in &subnets[1..] {
                shared.retain(|offered| subnet.network_types.contains(offered));
            }
            assert_eq!(
                wire_group_network_types(reply),
                shared,
                "case {case} / {reader}: the group must offer what all of its subnets offer, \
                 got {:?}",
                wire_group_network_types(reply)
            );
            assert!(
                !shared.is_empty(),
                "case {case} / {reader}: a group of addressed subnets must offer a network \
                 type, got {subnets:?}"
            );
        }
    }
}

#[test]
fn the_network_types_a_subnet_offers_follow_from_its_address_families() {
    // The axis: the address families a subnet carries, over every combination of them. The
    // values and their order are the modelled `NetworkType` enum's (`ipv4 | ipv6 |
    // dual_stack`, from `smithy_query(query="shape", shape="NetworkType")`); EC2 addresses
    // every subnet it mints in this emulator in IPv4 alone, so the other three rows are the
    // ones only this table reaches.
    let rows: [(&str, bool, bool, &[&str]); 4] = [
        ("an IPv4-only subnet", true, false, &["ipv4"]),
        ("an IPv6-only subnet", false, true, &["ipv6"]),
        (
            "a dual-stack subnet",
            true,
            true,
            &["ipv4", "ipv6", "dual_stack"],
        ),
        ("a subnet with no block at all", false, false, &[]),
    ];
    for (case, ipv4, ipv6, expected) in rows {
        let offered = state::subnet_network_types(ipv4, ipv6);
        assert_eq!(
            offered, expected,
            "case {case}: expected {expected:?}, got {offered:?}"
        );
    }
}

#[test]
fn a_group_offers_only_the_network_types_every_one_of_its_subnets_offers() {
    // One subnet of a group, built from what it offers; the identifier and the zone play no
    // part in this rule.
    let subnet = |offers: &[&str]| state::SubnetRecord {
        subnet_identifier: format!("subnet-{}", offers.join("-")),
        availability_zone: ZONES[0].to_string(),
        supported_network_types: offers.iter().map(|offer| (*offer).to_string()).collect(),
    };
    // The axis: what the subnets of one group offer each other — all the same, one narrower
    // than the rest, nothing in common, and a group holding none at all.
    let rows: [(&str, Vec<state::SubnetRecord>, &[&str]); 5] = [
        ("one IPv4 subnet", vec![subnet(&["ipv4"])], &["ipv4"]),
        (
            "two IPv4 subnets",
            vec![subnet(&["ipv4"]), subnet(&["ipv4"])],
            &["ipv4"],
        ),
        (
            "two dual-stack subnets",
            vec![
                subnet(&["ipv4", "ipv6", "dual_stack"]),
                subnet(&["ipv4", "ipv6", "dual_stack"]),
            ],
            &["ipv4", "ipv6", "dual_stack"],
        ),
        (
            "a dual-stack subnet beside an IPv4-only one",
            vec![subnet(&["ipv4", "ipv6", "dual_stack"]), subnet(&["ipv4"])],
            &["ipv4"],
        ),
        (
            "an IPv4-only subnet beside an IPv6-only one",
            vec![subnet(&["ipv4"]), subnet(&["ipv6"])],
            &[],
        ),
    ];
    for (case, subnets, expected) in rows {
        let offered = state::group_network_types(&subnets);
        assert_eq!(
            offered, expected,
            "case {case}: expected {expected:?}, got {offered:?}"
        );
    }
}

#[tokio::test]
async fn subnets_spanning_two_vpcs_are_refused_as_an_invalid_subnet() {
    let net = network().await;
    let defaults = net.default_subnets.clone();
    let client = net.client_subnets.clone();

    // A group that exists, so the modify rows have something to modify.
    let existing = "spanning-under-test";
    let created = net
        .emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params(existing, "one VPC", &client[..2]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the group under test must be created, got {} {}",
        created.status, created.body
    );

    // The axis: which operation builds the group, how many subnets the list carries, and
    // which VPC the first of them sits in.
    let rows: Vec<(&str, Vec<String>)> = vec![
        (
            "a default-VPC subnet then a client one",
            vec![defaults[0].clone(), client[0].clone()],
        ),
        (
            "a client subnet then a default-VPC one",
            vec![client[0].clone(), defaults[0].clone()],
        ),
        (
            "two client subnets then a default-VPC one",
            vec![client[0].clone(), client[1].clone(), defaults[0].clone()],
        ),
    ];
    for (case, ids) in rows {
        // The two identifiers the refusal names: the first subnet of the list, whose VPC the
        // group would sit in, and the first one belonging to another.
        let first = ids[0].clone();
        let first_vpc = reported_vpc(&net.emulator, &first).await;
        let mut other = None;
        for id in &ids[1..] {
            if reported_vpc(&net.emulator, id).await != first_vpc {
                other = Some(id.clone());
                break;
            }
        }
        let other = other.unwrap_or_else(|| panic!("case {case}: the row must span two VPCs"));
        let expected = format!("Subnets {first} and {other} are not in the same VPC.");

        let name = format!("spanning-{first}");
        let created = net
            .emulator
            .call(
                "CreateCacheSubnetGroup",
                &subnet_group_params(&name, case, &ids),
            )
            .await;
        let mut modify = params(&[("CacheSubnetGroupName", existing)]);
        modify.extend(crate::cache_cluster_lifecycle::subnet_pairs(
            "SubnetIdentifier",
            &ids,
        ));
        let modified = net.emulator.call("ModifyCacheSubnetGroup", &modify).await;

        for (writer, refused) in [("create", &created), ("modify", &modified)] {
            assert_eq!(
                refused.status, 400,
                "case {case} / {writer}: subnets of two VPCs must be refused 400, got {} {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "InvalidSubnet",
                "case {case} / {writer}: expected InvalidSubnet, got {}",
                refused.body
            );
            assert_eq!(
                refused.error_message(),
                expected,
                "case {case} / {writer}: expected {expected:?}, got {:?}",
                refused.error_message()
            );
        }

        // Nothing was stored, and the group the modify named still holds what it held.
        let described = net
            .emulator
            .call(
                "DescribeCacheSubnetGroups",
                &params(&[("CacheSubnetGroupName", &name)]),
            )
            .await;
        assert_eq!(
            described.error_code(),
            "CacheSubnetGroupNotFoundFault",
            "case {case}: a refused create must store no group, got {}",
            described.body
        );
        let unchanged = net
            .emulator
            .call(
                "DescribeCacheSubnetGroups",
                &params(&[("CacheSubnetGroupName", existing)]),
            )
            .await;
        assert_eq!(
            wire_subnets(case, &unchanged)
                .iter()
                .map(|subnet| subnet.identifier.clone())
                .collect::<Vec<String>>(),
            client[..2].to_vec(),
            "case {case}: a refused modify must change nothing, got {}",
            unchanged.body
        );
    }
}

/// The VPC EC2 reports for `subnet_id`. ASKED, never assumed.
async fn reported_vpc(emulator: &Emulator, subnet_id: &str) -> String {
    emulator
        .ec2(REGION)
        .describe_subnets()
        .subnet_ids(subnet_id)
        .send()
        .await
        .unwrap_or_else(|error| panic!("EC2 must report the subnet {subnet_id}: {error:?}"))
        .subnets()
        .iter()
        .find(|subnet| subnet.subnet_id() == Some(subnet_id))
        .and_then(::aws_sdk_ec2::types::Subnet::vpc_id)
        .unwrap_or_else(|| panic!("EC2 must report a VPC for {subnet_id}"))
        .to_string()
}
