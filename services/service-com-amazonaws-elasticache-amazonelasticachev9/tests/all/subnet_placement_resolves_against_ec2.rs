// SPDX-License-Identifier: CC0-1.0

//! The ONE answer to "which subnets does this account have", and the sentence a placement
//! that names no cache subnet group is refused with.
//!
//! Two rules, each over a SET of inputs rather than one recorded request:
//!
//! 1. SUBNETS — a subnet is an Amazon EC2 resource, so every ElastiCache operation that
//!    places a resource in subnets (`CreateCacheSubnetGroup$SubnetIds`,
//!    `CreateServerlessCache$SubnetIds`) resolves each identifier it is given against what
//!    EC2 holds for the account in the Region the request is signed for: an identifier EC2
//!    reports a subnet for is accepted and what the resource reports about the placement
//!    (the Availability Zone, the VPC) is read off EC2's own record, and an identifier it
//!    reports none for — one never minted, or one minted and then deleted — is refused
//!    `InvalidParameterValue` with nothing stored. The rows differ along what the rule
//!    generalises over: whether the subnet is one of the default VPC's or one a client
//!    minted with `ec2:CreateSubnet`, how many are named, the order and position they are
//!    named in, and whether an absent identifier was never minted or was deleted.
//!
//! 2. WORDING — the refusal an operation that would PLACE a resource in a cache subnet
//!    group answers for a name this account has no group under quotes that name:
//!    `Cache subnet group '<name>' does not exist.`, naming the name's STORED identity —
//!    `CacheSubnetGroupName` "is stored as a lowercase string", so the service names a
//!    group under that one identity whatever case the request spelled. The rows span the
//!    names a request may send and both operations that take a `CacheSubnetGroupName` to
//!    place a resource in, with the operations that address the GROUP itself as the
//!    contrasting rows: each of those words the same fault its own way, so the template
//!    belongs to the placement.
//!
//! EC2 is mounted beside ElastiCache on ONE engine by [`crate::harness`] and driven through
//! the AWS SDK over the same in-process callback this service's own resolution reaches it
//! through. Nothing here reaches into either store: a subnet exists only because EC2
//! answered for it, and a group only because a later `DescribeCacheSubnetGroups` reported
//! it. Every identifier a row names was MINTED by EC2 or is deliberately absent, so no id
//! this suite writes can pass itself off as one the account holds.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::cache_cluster_lifecycle::{subnet_group_params, subnet_pairs};
use crate::harness::{params, Emulator, REGION};

/// The Availability Zones the client-minted subnets below sit in — one per subnet, so a row
/// that names two subnets names two zones. Written as `<region><letter>`, the form AWS names
/// a zone of a Region in.
const ZONES: [&str; 3] = ["us-east-1a", "us-east-1b", "us-east-1c"];

/// Identifiers of the EC2 subnet form (`subnet-` and hex) that EC2 mints no subnet for, so
/// each reaches the handler's resolution and resolves to nothing. Both the short and the long
/// form AWS has used for a subnet id appear.
const ABSENT_SUBNET: &str = "subnet-00000000deadbeef";
const SHORT_ABSENT_SUBNET: &str = "subnet-0000beef";

/// One placement row: the operation, the rest of a request it accepts, and the element its
/// answer reports the group it was placed in under, where the modelled output carries one.
type PlacementCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    Option<&'static str>,
);

/// The account's network for one case: the emulator, the default subnets EC2 reports for
/// [`REGION`], the subnets a client minted in a VPC of its own, and one more it minted and
/// then deleted.
struct Network {
    emulator: Emulator,
    /// The default VPC's default subnets, as EC2 reports them, in zone order.
    default_subnets: Vec<String>,
    /// The subnets `ec2:CreateSubnet` minted in the client's own VPC, one per zone of
    /// [`ZONES`].
    client_subnets: Vec<String>,
    /// The VPC the client's subnets sit in, as EC2 reported it.
    client_vpc: String,
    /// A subnet of the client's VPC that EC2 minted and then deleted: an id the account HELD
    /// and holds no longer, which is a different thing from one it never had.
    deleted_subnet: String,
}

/// Stand up one emulator and build the account's network through EC2's own API.
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
        .cidr_block("10.9.0.0/16")
        .send()
        .await
        .expect("EC2 must create the client's VPC")
        .vpc()
        .and_then(::aws_sdk_ec2::types::Vpc::vpc_id)
        .expect("CreateVpc must report the VPC it minted")
        .to_string();

    let mut client_subnets = Vec::with_capacity(ZONES.len());
    for (index, zone) in ZONES.iter().enumerate() {
        client_subnets.push(client_subnet(&ec2, &client_vpc, index, zone).await);
    }
    let deleted_subnet = client_subnet(&ec2, &client_vpc, ZONES.len(), ZONES[0]).await;
    ec2.delete_subnet()
        .subnet_id(&deleted_subnet)
        .send()
        .await
        .expect("EC2 must delete the subnet it minted");

    Network {
        emulator,
        default_subnets,
        client_subnets,
        client_vpc,
        deleted_subnet,
    }
}

/// One subnet `ec2:CreateSubnet` mints in `vpc_id`, as the id EC2 reported.
async fn client_subnet(
    ec2: &::aws_sdk_ec2::Client,
    vpc_id: &str,
    index: usize,
    zone: &str,
) -> String {
    ec2.create_subnet()
        .vpc_id(vpc_id)
        .cidr_block(format!("10.9.{}.0/24", index + 1))
        .availability_zone(zone)
        .send()
        .await
        .unwrap_or_else(|error| panic!("EC2 must create a subnet in {zone}: {error:?}"))
        .subnet()
        .and_then(::aws_sdk_ec2::types::Subnet::subnet_id)
        .unwrap_or_else(|| panic!("CreateSubnet must report the subnet it minted in {zone}"))
        .to_string()
}

/// The Availability Zone and VPC EC2 reports for `subnet_id`, which is what a group holding
/// it must report about it. ASKED, never assumed.
async fn reported_placement(emulator: &Emulator, subnet_id: &str) -> (String, String) {
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
    (
        subnet
            .availability_zone()
            .unwrap_or_else(|| panic!("EC2 must report a zone for {subnet_id}"))
            .to_string(),
        subnet
            .vpc_id()
            .unwrap_or_else(|| panic!("EC2 must report a VPC for {subnet_id}"))
            .to_string(),
    )
}

#[tokio::test]
async fn a_cache_subnet_group_holds_only_subnets_the_owner_of_subnets_reports() {
    let net = network().await;
    let defaults = net.default_subnets.clone();
    let client = net.client_subnets.clone();
    // Each row: the case, the ids the request names, and whether the account has every one
    // of them.
    let rows: Vec<(&str, Vec<String>, bool)> = vec![
        (
            "one default subnet of the account",
            vec![defaults[0].clone()],
            true,
        ),
        ("every default subnet", defaults.clone(), true),
        (
            "one subnet a client minted in its own VPC",
            vec![client[0].clone()],
            true,
        ),
        (
            "two client subnets, two zones",
            vec![client[0].clone(), client[1].clone()],
            true,
        ),
        (
            "client subnets named in reverse zone order",
            vec![client[2].clone(), client[0].clone()],
            true,
        ),
        (
            "three client subnets",
            vec![client[0].clone(), client[1].clone(), client[2].clone()],
            true,
        ),
        (
            "an id EC2 never minted",
            vec![ABSENT_SUBNET.to_string()],
            false,
        ),
        (
            "a shorter id EC2 never minted",
            vec![SHORT_ABSENT_SUBNET.to_string()],
            false,
        ),
        (
            "an id EC2 minted and then deleted",
            vec![net.deleted_subnet.clone()],
            false,
        ),
        (
            "a real id FIRST, an absent one after it",
            vec![client[0].clone(), ABSENT_SUBNET.to_string()],
            false,
        ),
        (
            "an absent id FIRST, a real one after it",
            vec![ABSENT_SUBNET.to_string(), client[1].clone()],
            false,
        ),
        (
            "two absent ids",
            vec![ABSENT_SUBNET.to_string(), SHORT_ABSENT_SUBNET.to_string()],
            false,
        ),
    ];

    let held: Vec<String> = defaults.iter().chain(client.iter()).cloned().collect();
    for (index, (case, subnet_ids, accepted)) in rows.iter().enumerate() {
        let name = format!("owner-resolved-{index}");
        let created = net
            .emulator
            .call(
                "CreateCacheSubnetGroup",
                &subnet_group_params(&name, "the subnet-owner suite", subnet_ids),
            )
            .await;
        let described = net
            .emulator
            .call(
                "DescribeCacheSubnetGroups",
                &params(&[("CacheSubnetGroupName", &name)]),
            )
            .await;

        if !*accepted {
            assert_eq!(
                created.status, 400,
                "case {case:?}: an id the account has no subnet for must be refused 400, got {} \
                 {}",
                created.status, created.body
            );
            assert_eq!(
                created.error_code(),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {}",
                created.body
            );
            // The refusal names exactly the ids that resolved to nothing.
            let message = created.error_message().to_string();
            for id in subnet_ids {
                assert_eq!(
                    message.contains(id.as_str()),
                    !held.contains(id),
                    "case {case:?}: the message must name {id:?} only when the account has no \
                     subnet for it, got {message:?}"
                );
            }
            assert_eq!(
                described.error_code(),
                "CacheSubnetGroupNotFoundFault",
                "case {case:?}: a refused create must store no group, got {}",
                described.body
            );
            continue;
        }

        assert_eq!(
            created.status, 200,
            "case {case:?}: subnets the account has must be accepted, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            described.all("Subnets/Subnet/SubnetIdentifier"),
            subnet_ids.iter().map(String::as_str).collect::<Vec<&str>>(),
            "case {case:?}: the stored group must hold the named subnets in order, got {}",
            described.body
        );
        // What the group reports about the placement is the owner's answer, not a value
        // built from the Region or the request.
        let mut expected_zones: Vec<String> = Vec::with_capacity(subnet_ids.len());
        let mut expected_vpcs: Vec<String> = Vec::with_capacity(subnet_ids.len());
        for id in subnet_ids {
            let (zone, vpc) = reported_placement(&net.emulator, id).await;
            expected_zones.push(zone);
            expected_vpcs.push(vpc);
        }
        assert_eq!(
            described.all("SubnetAvailabilityZone/Name"),
            expected_zones
                .iter()
                .map(String::as_str)
                .collect::<Vec<&str>>(),
            "case {case:?}: every subnet must report the zone EC2 reports for it, got {:?}",
            described.all("SubnetAvailabilityZone/Name")
        );
        assert_eq!(
            described.text("CacheSubnetGroup/VpcId"),
            Some(expected_vpcs[0].as_str()),
            "case {case:?}: the group must report the VPC EC2 reports for its subnets, got {:?}",
            described.text("CacheSubnetGroup/VpcId")
        );
    }

    // The contrast the rule rests on: a group built on the client's own subnets reports the
    // client's VPC, which is NOT the default VPC the account's default subnets sit in — so
    // the VPC a group reports cannot have been derived from the Region.
    let (_, default_vpc) = reported_placement(&net.emulator, &net.default_subnets[0]).await;
    assert_ne!(
        net.client_vpc, default_vpc,
        "the client's VPC must differ from the default VPC, got {:?} and {default_vpc:?}",
        net.client_vpc
    );

    // And because the two VPCs are different, a request naming a subnet of each names
    // subnets that do not match each other: that is refused with the fault the model
    // declares for it, and nothing is stored — rather than accepted with one of the two
    // VPCs reported. Both orders, so the answer does not depend on which came first.
    for (case, ids) in [
        (
            "a default-VPC subnet and a client-VPC subnet",
            vec![net.default_subnets[0].clone(), client[0].clone()],
        ),
        (
            "a client-VPC subnet and a default-VPC subnet",
            vec![client[0].clone(), net.default_subnets[0].clone()],
        ),
    ] {
        let name = format!("mixed-vpc-{}", ids[0]);
        // Which identifier belongs where, as EC2 reports it — the variable parts of both
        // sentences below.
        let (_, first_vpc) = reported_placement(&net.emulator, &ids[0]).await;
        let (_, second_vpc) = reported_placement(&net.emulator, &ids[1]).await;
        let refused = net
            .emulator
            .call(
                "CreateCacheSubnetGroup",
                &subnet_group_params(&name, "subnets of two VPCs", &ids),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: subnets of two VPCs must be refused 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidSubnet",
            "case {case:?}: expected the mismatched-subnet fault, got {}",
            refused.body
        );
        // The service's sentence for subnets of two VPCs, naming the two identifiers that
        // disagree in the order the request listed them.
        let _ = (&first_vpc, &second_vpc);
        assert_eq!(
            refused.error_message(),
            format!("Subnets {} and {} are not in the same VPC.", ids[0], ids[1]),
            "case {case:?}: expected the not-in-one-VPC sentence naming both ids, got {:?}",
            refused.error_message()
        );
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
            "case {case:?}: a refused create must store no group, got {}",
            described.body
        );

        // The same rule at the other operation that takes subnet ids, where the member
        // documents it itself ("All the subnetIds must belong to the same VPC").
        let mut pairs = params(&[
            ("ServerlessCacheName", name.as_str()),
            ("Engine", "redis"),
        ]);
        pairs.extend(subnet_pairs("SubnetId", &ids));
        let refused = net.emulator.call("CreateServerlessCache", &pairs).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} (serverless): subnets of two VPCs must be refused 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?} (serverless): expected InvalidParameterValue, got {}",
            refused.body
        );
        // The member's own sentence ("All the subnetIds must belong to the same VPC"), then
        // which identifier belongs where.
        assert_eq!(
            refused.error_message(),
            format!(
                "All the subnetIds must belong to the same VPC: {} belongs to {second_vpc} and \
                 {} belongs to {first_vpc}.",
                ids[1], ids[0]
            ),
            "case {case:?} (serverless): expected the member's documented rule with the VPCs \
             EC2 reports, got {:?}",
            refused.error_message()
        );
    }
}

#[tokio::test]
async fn a_serverless_cache_is_placed_only_in_subnets_the_owner_of_subnets_reports() {
    // The same rule at the other operation that takes subnet ids, over the same axes: a
    // client-minted subnet, several of them, one deleted again, and one never minted.
    let net = network().await;
    let client = net.client_subnets.clone();
    let rows: Vec<(&str, Vec<String>, bool)> = vec![
        (
            "one subnet a client minted",
            vec![client[0].clone()],
            true,
        ),
        (
            "two client subnets, two zones",
            vec![client[0].clone(), client[1].clone()],
            true,
        ),
        (
            "one default subnet of the account",
            vec![net.default_subnets[0].clone()],
            true,
        ),
        (
            "an id EC2 minted and then deleted",
            vec![net.deleted_subnet.clone()],
            false,
        ),
        (
            "a real id and an id EC2 never minted",
            vec![client[0].clone(), ABSENT_SUBNET.to_string()],
            false,
        ),
    ];

    for (index, (case, subnet_ids, accepted)) in rows.iter().enumerate() {
        let name = format!("owner-placed-{index}");
        let mut pairs = params(&[("ServerlessCacheName", &name), ("Engine", "redis")]);
        pairs.extend(subnet_pairs("SubnetId", subnet_ids));
        let created = net.emulator.call("CreateServerlessCache", &pairs).await;
        let described = net
            .emulator
            .call(
                "DescribeServerlessCaches",
                &params(&[("ServerlessCacheName", &name)]),
            )
            .await;
        if *accepted {
            assert_eq!(
                created.status, 200,
                "case {case:?}: subnets the account has must be accepted, got {} {}",
                created.status, created.body
            );
            assert_eq!(
                described.all("SubnetId"),
                subnet_ids.iter().map(String::as_str).collect::<Vec<&str>>(),
                "case {case:?}: the stored cache must hold the named subnets, got {}",
                described.body
            );
            continue;
        }
        assert_eq!(
            created.status, 400,
            "case {case:?}: must be refused 400, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {}",
            created.body
        );
        assert_eq!(
            described.error_code(),
            "ServerlessCacheNotFoundFault",
            "case {case:?}: a refused create must store no cache, got {}",
            described.body
        );
    }

    // The placement is bounded before anything is resolved: one past this
    // service's own bound on how many subnets one placement holds is refused for
    // the COUNT, naming it, rather than for the ids.
    let too_many: Vec<String> = (0..=state::SUBNETS_PER_GROUP)
        .map(|index| format!("subnet-{index:017x}"))
        .collect();
    let mut pairs = params(&[("ServerlessCacheName", "owner-placed-wide"), ("Engine", "redis")]);
    pairs.extend(subnet_pairs("SubnetId", &too_many));
    let refused = net.emulator.call("CreateServerlessCache", &pairs).await;
    assert_eq!(
        refused.status, 400,
        "case one subnet past the bound: must be refused 400, got {} {}",
        refused.status, refused.body
    );
    assert!(
        refused
            .error_message()
            .contains(&too_many.len().to_string()),
        "case one subnet past the bound: the refusal must name the count supplied, got {:?}",
        refused.error_message()
    );
}

#[tokio::test]
async fn a_placement_naming_no_cache_subnet_group_quotes_the_name_it_was_given() {
    let net = network().await;
    // A group that DOES exist, so the refusals below are about resolution and the accepting
    // rows show the same member resolving.
    let existing = "placement-group";
    let created = net
        .emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params(
                existing,
                "the placement-wording suite",
                &net.client_subnets[..2],
            ),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the subnet group must be created, got {} {}",
        created.status, created.body
    );

    // The names a request may send: the shortest, one with hyphens, one whose case differs
    // from the stored (lower-cased) identity, and the longest the member's documented bound
    // admits ("must contain no more than 255 alphanumeric characters or hyphens").
    let longest = "g".repeat(255);
    let missing_names: [&str; 5] = [
        "g",
        "no-such-group",
        "Mixed-Case-Group",
        "group-0123456789",
        longest.as_str(),
    ];
    // Each operation that PLACES a resource in a cache subnet group, with the rest of a
    // request it accepts and the element its answer reports the group it was placed in
    // under (`ReplicationGroup` models no member for it, so the group it is placed in is
    // read back off its member cluster instead).
    let placements: [PlacementCase; 2] = [
        (
            "CreateCacheCluster",
            &[
                ("CacheClusterId", "placed-cluster"),
                ("CacheNodeType", "cache.t3.micro"),
            ],
            Some("CacheSubnetGroupName"),
        ),
        (
            "CreateReplicationGroup",
            &[
                ("ReplicationGroupId", "placed-group"),
                ("ReplicationGroupDescription", "a placed replication group"),
                ("CacheNodeType", "cache.t3.micro"),
            ],
            None,
        ),
    ];

    for (action, rest, reports_group_under) in placements {
        for name in missing_names {
            let mut pairs = params(rest);
            pairs.push(("CacheSubnetGroupName".to_string(), name.to_string()));
            let refused = net.emulator.call(action, &pairs).await;
            assert_eq!(
                refused.status, 400,
                "case {action} / {name:?}: expected HTTP 400, got {} {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                "CacheSubnetGroupNotFoundFault",
                "case {action} / {name:?}: expected CacheSubnetGroupNotFoundFault, got {}",
                refused.body
            );
            assert_eq!(
                refused.error_message(),
                format!(
                    "Cache subnet group '{}' does not exist.",
                    name.to_ascii_lowercase()
                ),
                "case {action} / {name:?}: the refusal must quote the name's stored identity, \
                 got {:?}",
                refused.error_message()
            );
        }

        // The accepting half: the same member naming the group that exists resolves, so the
        // refusal above is about resolution and not about the member.
        let mut pairs = params(rest);
        pairs.push(("CacheSubnetGroupName".to_string(), existing.to_string()));
        let accepted = net.emulator.call(action, &pairs).await;
        assert_eq!(
            accepted.status, 200,
            "case {action} / a group that exists: expected HTTP 200, got {} {}",
            accepted.status, accepted.body
        );
        if let Some(member) = reports_group_under {
            assert_eq!(
                accepted.text(member),
                Some(existing),
                "case {action} / a group that exists: the resource must report the group it is \
                 placed in, got {}",
                accepted.body
            );
        }
    }

    // The replication group's own member cluster is where its placement is read back from,
    // because the modelled `ReplicationGroup` carries no `CacheSubnetGroupName`.
    let member = net
        .emulator
        .call(
            "DescribeCacheClusters",
            &params(&[("CacheClusterId", "placed-group-001")]),
        )
        .await;
    assert_eq!(
        member.text("CacheSubnetGroupName"),
        Some(existing),
        "the replication group's member cluster must report the group it was placed in, got {}",
        member.body
    );

    // The contrast: an operation that addresses the GROUP ITSELF words the same fault its
    // own way, so the quoted template belongs to the placement rather than to the fault.
    for name in missing_names {
        let described = net
            .emulator
            .call(
                "DescribeCacheSubnetGroups",
                &params(&[("CacheSubnetGroupName", name)]),
            )
            .await;
        assert_eq!(
            described.error_code(),
            "CacheSubnetGroupNotFoundFault",
            "case describe / {name:?}: expected CacheSubnetGroupNotFoundFault, got {}",
            described.body
        );
        assert_eq!(
            described.error_message(),
            format!(
                "Cache subnet group {} not found.",
                name.to_ascii_lowercase()
            ),
            "case describe / {name:?}: the describe keeps its own wording, got {:?}",
            described.error_message()
        );

        let deleted = net
            .emulator
            .call(
                "DeleteCacheSubnetGroup",
                &params(&[("CacheSubnetGroupName", name)]),
            )
            .await;
        assert_eq!(
            deleted.error_code(),
            "CacheSubnetGroupNotFoundFault",
            "case delete / {name:?}: expected CacheSubnetGroupNotFoundFault, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.error_message(),
            format!(
                "Cache Subnet Group {} does not exist.",
                name.to_ascii_lowercase()
            ),
            "case delete / {name:?}: the delete keeps its own wording, got {:?}",
            deleted.error_message()
        );
    }
}
