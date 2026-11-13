// SPDX-License-Identifier: CC0-1.0

//! EC2 state survives a restart through `service_common::persist`: a VPC, a subnet in it and a
//! security group with an ingress rule read back unchanged from a fresh engine restored from
//! the snapshot. The VPC's `pending` -> `available` deadline is a monotonic instant and its
//! state a catalog word, the two shapes `persist::instant_map` and `persist::restore_static`
//! exist for.

use service_com_amazonaws_ec2_amazonec2::state::ComAmazonawsEc2Amazonec2State as State;
use service_common::persist::Persisted;

use super::ec2_query::{call, element, elements, emulator};

const REGION: &str = "us-east-1";

fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[tokio::test]
async fn a_network_from_before_a_restart_reads_back_after_it() {
    let (before, engine) = emulator();
    let (status, body) = call(&before, REGION, "CreateVpc", &params(&[("CidrBlock", "10.20.0.0/16")])).await;
    assert_eq!(status, 200, "{body}");
    let vpc = element(&body, "vpcId").unwrap();
    let (status, body) = call(
        &before,
        REGION,
        "CreateSubnet",
        &params(&[("VpcId", &vpc), ("CidrBlock", "10.20.1.0/24")]),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let subnet = element(&body, "subnetId").unwrap();
    let (status, body) = call(
        &before,
        REGION,
        "CreateSecurityGroup",
        &params(&[("GroupName", "web"), ("GroupDescription", "web tier"), ("VpcId", &vpc)]),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let group = element(&body, "groupId").unwrap();
    let (status, body) = call(
        &before,
        REGION,
        "AuthorizeSecurityGroupIngress",
        &params(&[
            ("GroupId", &group),
            ("IpPermissions.1.IpProtocol", "tcp"),
            ("IpPermissions.1.FromPort", "443"),
            ("IpPermissions.1.ToPort", "443"),
            ("IpPermissions.1.IpRanges.1.CidrIp", "0.0.0.0/0"),
        ]),
    )
    .await;
    assert_eq!(status, 200, "{body}");

    // What a shutdown writes, through the same JSON text the file holds.
    let snapshot = engine.state::<State>().lock().snapshot();
    let text = serde_json::to_string(&snapshot).unwrap();
    let restored = State::restore(State::VERSION, serde_json::from_str(&text).unwrap()).unwrap();

    let (after, fresh) = emulator();
    *fresh.state::<State>().lock() = restored;
    let (status, body) = call(&after, REGION, "DescribeVpcs", &params(&[("VpcId.1", &vpc)])).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(element(&body, "cidrBlock").as_deref(), Some("10.20.0.0/16"));
    let (status, body) =
        call(&after, REGION, "DescribeSubnets", &params(&[("SubnetId.1", &subnet)])).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(element(&body, "vpcId").as_deref(), Some(vpc.as_str()));
    let (status, body) =
        call(&after, REGION, "DescribeSecurityGroups", &params(&[("GroupId.1", &group)])).await;
    assert_eq!(status, 200, "{body}");
    assert!(elements(&body, "fromPort").contains(&"443".to_string()), "{body}");
}
