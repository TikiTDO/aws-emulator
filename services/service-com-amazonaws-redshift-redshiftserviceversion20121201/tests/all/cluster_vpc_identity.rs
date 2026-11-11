// SPDX-License-Identifier: CC0-1.0

//! A cluster's VPC is RESOLVED from the subnets its subnet group is made of,
//! never synthesized from a name.
//!
//! The RULE every case here asserts: the VPC a Redshift resource reports
//! (`ClusterSubnetGroup$VpcId`, `Cluster$VpcId`, and the `VpcId` of the VPC
//! interface endpoint a serving cluster is reached through) is the VPC that
//! CONTAINS the subnets the group was created over, exactly as the service that
//! owns subnets reports it — so the id a Redshift read answers with is one
//! `ec2:DescribeVpcs` resolves, and two clusters are in one VPC only when their
//! subnets are.
//!
//! Nothing here is computed from a Redshift name: a VPC id derived from a group
//! name would still be a well-formed id, which is why each case cross-checks the
//! id against EC2 rather than against a formula, and why the rows pin both
//! directions of the identity — different VPCs give different ids, and
//! different group names over ONE VPC give the same id.
//!
//! EC2 is mounted beside Redshift on ONE engine (the shared harness) and driven
//! through the AWS SDK over the same in-process callback this service's own
//! resolution reaches it through. Every id a row names was MINTED by EC2.
//!
//! The rows differ along what the rule generalises over: how many subnets the
//! group covers and how many zones they span, whether the VPC is one the client
//! minted or the account's own default VPC, how many groups share a VPC, and
//! whether the cluster named a subnet group at all.

use std::time::Duration;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};

/// The admin password every cluster in this suite is created with.
const PASSWORD: &str = "TestPassw0rd123";

/// The node type every cluster in this suite is created with.
const NODE_TYPE: &str = "ra3.xlplus";

/// `CreateClusterSubnetGroup` over `subnet_ids`.
async fn create_group(emulator: &Emulator, name: &str, subnet_ids: &[String]) -> Answer {
    let mut params: Vec<(String, String)> = vec![
        ("ClusterSubnetGroupName".to_string(), name.to_string()),
        ("Description".to_string(), format!("subnets of {name}")),
    ];
    for (index, subnet_id) in subnet_ids.iter().enumerate() {
        params.push((
            format!("SubnetIds.SubnetIdentifier.{}", index + 1),
            subnet_id.clone(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator.call("CreateClusterSubnetGroup", &borrowed).await
}

/// `CreateCluster` for `identifier`, into `subnet_group` when one is named.
async fn create_cluster(
    emulator: &Emulator,
    identifier: &str,
    subnet_group: Option<&str>,
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", identifier),
        ("NodeType", NODE_TYPE),
        ("MasterUsername", "adminuser"),
        ("MasterUserPassword", PASSWORD),
    ];
    if let Some(group) = subnet_group {
        params.push(("ClusterSubnetGroupName", group));
    }
    emulator.call("CreateCluster", &params).await
}

/// The ONE VPC EC2 reports for `subnet_ids` — asked of EC2, never assumed.
async fn vpc_of_subnets(emulator: &Emulator, case: &str, subnet_ids: &[String]) -> String {
    let described = emulator
        .ec2()
        .describe_subnets()
        .set_subnet_ids(Some(subnet_ids.to_vec()))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case}: EC2 must report the subnets it minted: {error:?}")
        });
    let mut reported: Vec<String> = described
        .subnets()
        .iter()
        .filter_map(|subnet| subnet.vpc_id().map(str::to_string))
        .collect();
    reported.sort();
    reported.dedup();
    assert_eq!(
        reported.len(),
        1,
        "case {case}: the suite's own subnets must lie in exactly one VPC, got {reported:?}"
    );
    reported.remove(0)
}

/// The account's DEFAULT VPC in the Region, as EC2 reports it (`is-default`).
async fn default_vpc(emulator: &Emulator, case: &str) -> String {
    emulator
        .ec2()
        .describe_vpcs()
        .filters(
            aws_sdk_ec2::types::Filter::builder()
                .name("is-default")
                .values("true")
                .build(),
        )
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case}: EC2 must report the account's default VPC: {error:?}")
        })
        .vpcs()
        .iter()
        .find_map(|vpc| vpc.vpc_id().map(str::to_string))
        .unwrap_or_else(|| panic!("case {case}: the account must have a default VPC in the Region"))
}

/// Whether EC2 holds a VPC with this id: a synthesized id resolves to nothing.
async fn ec2_holds_vpc(emulator: &Emulator, vpc_id: &str) -> bool {
    emulator
        .ec2()
        .describe_vpcs()
        .vpc_ids(vpc_id)
        .send()
        .await
        .map(|described| {
            described
                .vpcs()
                .iter()
                .any(|vpc| vpc.vpc_id() == Some(vpc_id))
        })
        .unwrap_or(false)
}

/// Assert `answer` reports this one VPC and nothing else, wherever a `VpcId`
/// appears in it.
fn assert_every_vpc_is(case: &str, answer: &Answer, expected: &str) {
    let reported = all_text(&answer.body, "VpcId");
    assert!(
        !reported.is_empty() && reported.iter().all(|value| value == expected),
        "case {case}: expected every VpcId to be {expected:?}, got {reported:?}"
    );
}

/// A cluster launched into a subnet group reports the VPC EC2 places that
/// group's subnets in — an id EC2 itself resolves — and so does the group.
#[tokio::test]
async fn a_cluster_reports_the_vpc_ec2_places_its_subnet_group_in() {
    let emulator = Emulator::new();

    // The rows differ along how many subnets the group is made of, and so along
    // how many Availability Zones they span; each row's subnets are minted in a
    // VPC of their own.
    let rows: [(&str, usize); 3] = [
        ("one subnet, one zone", 1),
        ("three subnets", 3),
        ("eight subnets", 8),
    ];

    let mut reported_per_row: Vec<String> = Vec::new();
    for (index, (case, subnet_count)) in rows.into_iter().enumerate() {
        let subnets = emulator.subnet_ids(case, subnet_count).await;
        let expected = vpc_of_subnets(&emulator, case, &subnets).await;
        let group = format!("vpc-identity-{index:02}");
        let created_group = create_group(&emulator, &group, &subnets).await;
        assert_eq!(
            created_group.status, 200,
            "case {case}: expected the group create served, got {} with body {}",
            created_group.status, created_group.body
        );
        assert_eq!(
            created_group.text("VpcId").as_deref(),
            Some(expected.as_str()),
            "case {case}: expected the group in the VPC EC2 holds its subnets in {expected:?}, \
             got {:?}",
            created_group.text("VpcId")
        );

        let identifier = format!("vpc-identity-cluster-{index:02}");
        let created = create_cluster(&emulator, &identifier, Some(&group)).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected the cluster create served, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("VpcId").as_deref(),
            Some(expected.as_str()),
            "case {case}: expected the cluster in VPC {expected:?}, got {:?}",
            created.text("VpcId")
        );

        // The read path reports the same VPC the write stored, and that id is one
        // the owner of VPCs resolves: a value this service invented would not be.
        let described = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", &identifier)])
            .await;
        let reported = described.text("VpcId").unwrap_or_else(|| {
            panic!(
                "case {case}: expected the cluster to report a VpcId, got body {}",
                described.body
            )
        });
        assert_eq!(
            reported, expected,
            "case {case}: expected the cluster in VPC {expected:?}, got {reported:?}"
        );
        assert!(
            ec2_holds_vpc(&emulator, &reported).await,
            "case {case}: expected the cluster's VpcId {reported:?} to name a VPC EC2 holds"
        );
        reported_per_row.push(reported);
    }

    // Each row built its own VPC, so no two clusters report one: the id tracks
    // the network the subnets are in rather than anything about the names.
    let mut distinct = reported_per_row.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        reported_per_row.len(),
        "case clusters of different VPCs: expected one VpcId per VPC, got {reported_per_row:?}"
    );
}

/// Two differently named subnet groups over subnets of ONE VPC report THAT VPC,
/// and so do the clusters launched into them: the id is the subnets' VPC, not a
/// function of the group's name.
#[tokio::test]
async fn groups_sharing_a_vpc_report_the_same_vpc_whatever_they_are_named() {
    let emulator = Emulator::new();
    let case = "two groups, one VPC";
    let subnets = emulator.subnet_ids(case, 2).await;
    let expected = vpc_of_subnets(&emulator, case, &subnets).await;

    // The rows differ along the group's name and which subnet of the shared VPC
    // it covers — the only things that change between them.
    let rows: [(&str, &str, usize); 2] = [
        ("shared-vpc-alpha", "shared-alpha", 0),
        ("a-second-name-entirely", "shared-beta", 1),
    ];
    for (group, identifier, subnet) in rows {
        let created_group = create_group(&emulator, group, &subnets[subnet..=subnet]).await;
        assert_eq!(
            created_group.status, 200,
            "case {group}: expected the group create served, got {} with body {}",
            created_group.status, created_group.body
        );
        assert_eq!(
            created_group.text("VpcId").as_deref(),
            Some(expected.as_str()),
            "case {group}: expected the shared VPC {expected:?}, got {:?}",
            created_group.text("VpcId")
        );

        let created = create_cluster(&emulator, identifier, Some(group)).await;
        assert_eq!(
            created.status, 200,
            "case {group}: expected the cluster create served, got {} with body {}",
            created.status, created.body
        );
        assert_eq!(
            created.text("VpcId").as_deref(),
            Some(expected.as_str()),
            "case {group}: expected the cluster in the shared VPC {expected:?}, got {:?}",
            created.text("VpcId")
        );
    }
}

/// A cluster that names NO subnet group is launched into the account's ambient
/// `default` group, and reports the VPC EC2 reports as the account's default
/// one — including on the VPC interface endpoint it publishes once it serves.
#[tokio::test]
async fn a_cluster_with_no_subnet_group_reports_the_accounts_default_vpc() {
    let emulator = Emulator::new();
    let case = "no subnet group named";
    let identifier = "vpc-identity-default";

    let created = create_cluster(&emulator, identifier, None).await;
    assert_eq!(
        created.status, 200,
        "case {case}: expected the cluster create served, got {} with body {}",
        created.status, created.body
    );
    let expected = default_vpc(&emulator, case).await;
    assert_eq!(
        created.text("ClusterSubnetGroupName").as_deref(),
        Some(state::DEFAULT_CLUSTER_SUBNET_GROUP),
        "case {case}: expected the account's default subnet group, got {:?}",
        created.text("ClusterSubnetGroupName")
    );
    assert_eq!(
        created.text("VpcId").as_deref(),
        Some(expected.as_str()),
        "case {case}: expected the account's default VPC {expected:?}, got {:?}",
        created.text("VpcId")
    );

    // The group's own read agrees: one VPC, reported by both records.
    let group = emulator
        .call(
            "DescribeClusterSubnetGroups",
            &[(
                "ClusterSubnetGroupName",
                state::DEFAULT_CLUSTER_SUBNET_GROUP,
            )],
        )
        .await;
    assert_every_vpc_is(&format!("{case}, the default group"), &group, &expected);

    // Once the cluster is serving, the VPC interface endpoint it is reached
    // through is in that same VPC: every `VpcId` of the answer is the one id.
    tokio::time::sleep(state::PROVISIONING_SPAN + Duration::from_millis(250)).await;
    let serving = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
        .await;
    assert!(
        serving.text("VpcEndpointId").is_some(),
        "case {case}: expected a VPC endpoint once serving, got body {}",
        serving.body
    );
    assert_every_vpc_is(&format!("{case}, serving"), &serving, &expected);
    assert!(
        ec2_holds_vpc(&emulator, &expected).await,
        "case {case}: expected the reported VpcId {expected:?} to name a VPC EC2 holds"
    );
}
