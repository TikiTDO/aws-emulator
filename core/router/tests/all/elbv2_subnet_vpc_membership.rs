// SPDX-License-Identifier: CC0-1.0

//! Every subnet a load balancer is placed in belongs to ONE VPC — the load balancer's.
//!
//! A load balancer's network interfaces live in the subnets it is placed in and a network
//! interface belongs to exactly one VPC, so a placement that names a subnet of another VPC
//! is a configuration no load balancer can hold: `SetSubnets` refuses it with the modelled
//! `InvalidConfigurationRequest` (HTTP 400) and the load balancer keeps the subnets it had,
//! and `CreateLoadBalancer` refuses it and creates nothing.
//!
//! Which VPC a subnet is in is EC2's to say, so the rule is driven with both services on
//! one engine, here beside `elbv2_network_resolution.rs` (whose request plumbing this suite
//! reuses) rather than in the ELBv2 crate.
//!
//! The table varies what the rule generalises over: how many subnets the request names,
//! which of them is the foreign one and in which position, whether the placement is spelled
//! as `Subnets` or as `SubnetMappings`, the load balancer family (so a one-subnet placement
//! is covered too), and whether the call is the create or the replacement. Every mixed row
//! names one subnet per Availability Zone, so VPC membership is the only rule it breaks.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_network_resolution::{ec2_ok, elbv2, Answer, BOOT_STACK_SIZE};
use crate::query_wire::{element, elements};

/// The awsQuery code of `InvalidConfigurationRequestException`, which the model binds to
/// HTTP 400 (`smithy_query(query="operation",operation="SetSubnets")`).
const INVALID_CONFIGURATION_REQUEST: &str = "InvalidConfigurationRequest";

/// Create one VPC with a subnet per zone named, and answer `(vpc id, subnet ids)`.
fn vpc_with_subnets(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    cidr_prefix: &str,
    zones: &[&str],
) -> (String, Vec<String>) {
    let vpc = element(
        &ec2_ok(
            router,
            runtime,
            "CreateVpc",
            &[("CidrBlock", &format!("{cidr_prefix}.0.0/16"))],
        ),
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (index, zone) in zones.iter().enumerate() {
        subnets.push(element(
            &ec2_ok(
                router,
                runtime,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", &format!("{cidr_prefix}.{}.0/24", index + 1)),
                    ("AvailabilityZone", zone),
                ],
            ),
            "subnetId",
        ));
    }
    (vpc, subnets)
}

/// How a row spells the placement it asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// `Subnets.member.N`.
    Subnets,
    /// `SubnetMappings.member.N.SubnetId` — the other list the operation takes.
    Mappings,
}

/// The parameters one placement is spelled as, beside `LoadBalancerArn` or `Name`.
fn placement(spelling: Spelling, subnets: &[String]) -> Vec<(String, String)> {
    subnets
        .iter()
        .enumerate()
        .map(|(index, subnet)| {
            let position = index + 1;
            let key = match spelling {
                Spelling::Subnets => format!("Subnets.member.{position}"),
                Spelling::Mappings => format!("SubnetMappings.member.{position}.SubnetId"),
            };
            (key, subnet.clone())
        })
        .collect()
}

/// Drive one request whose parameters are owned strings.
fn call(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(String, String)],
) -> Answer {
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    elbv2(router, runtime, action, &borrowed)
}

/// The subnet ids `DescribeLoadBalancers` reports for the load balancer `arn` names.
fn placed_subnets(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    arn: &str,
) -> Vec<String> {
    let read = elbv2(
        router,
        runtime,
        "DescribeLoadBalancers",
        &[("LoadBalancerArns.member.1", arn)],
    );
    assert_eq!(
        read.status, 200,
        "the load balancer must be readable back, got {read:?}"
    );
    let mut subnets = elements(&read.text, "SubnetId");
    subnets.sort();
    subnets
}

/// One row of the replacement table.
struct Row {
    case: &'static str,
    /// Which load balancer the row replaces the subnets of: an index into the ARNs the
    /// suite created (0 — the Application Load Balancer, 1 — the Network Load Balancer).
    load_balancer: usize,
    spelling: Spelling,
    /// The subnets the request names, as `(vpc, index)` pairs resolved below: `true` means
    /// a subnet of the load balancer's own VPC.
    named: &'static [(bool, usize)],
    /// Whether the placement is accepted.
    accepted: bool,
}

/// Every row of the replacement table, in the order it is driven: each refusal is followed
/// by an accepted placement, so a refused call is shown to leave the operation working.
const ROWS: &[Row] = &[
    Row {
        case: "two subnets of the load balancer's own VPC",
        load_balancer: 0,
        spelling: Spelling::Subnets,
        named: &[(true, 0), (true, 1)],
        accepted: true,
    },
    Row {
        case: "one own subnet and one of another VPC",
        load_balancer: 0,
        spelling: Spelling::Subnets,
        named: &[(true, 0), (false, 0)],
        accepted: false,
    },
    Row {
        case: "the subnet of another VPC named first",
        load_balancer: 0,
        spelling: Spelling::Subnets,
        named: &[(false, 0), (true, 1)],
        accepted: false,
    },
    Row {
        case: "two subnets of another VPC",
        load_balancer: 0,
        spelling: Spelling::Subnets,
        named: &[(false, 0), (false, 1)],
        accepted: false,
    },
    Row {
        case: "another pair of the own VPC's subnets",
        load_balancer: 0,
        spelling: Spelling::Subnets,
        named: &[(true, 1), (true, 2)],
        accepted: true,
    },
    Row {
        case: "three subnets, the last of another VPC",
        load_balancer: 0,
        spelling: Spelling::Subnets,
        named: &[(true, 0), (true, 1), (false, 0)],
        accepted: false,
    },
    Row {
        case: "three subnets of the own VPC",
        load_balancer: 0,
        spelling: Spelling::Subnets,
        named: &[(true, 0), (true, 1), (true, 2)],
        accepted: true,
    },
    Row {
        case: "a mapping naming a subnet of another VPC",
        load_balancer: 0,
        spelling: Spelling::Mappings,
        named: &[(true, 0), (false, 0)],
        accepted: false,
    },
    Row {
        case: "mappings naming the own VPC's subnets",
        load_balancer: 0,
        spelling: Spelling::Mappings,
        named: &[(true, 0), (true, 1)],
        accepted: true,
    },
    Row {
        case: "one subnet of another VPC on a network load balancer",
        load_balancer: 1,
        spelling: Spelling::Subnets,
        named: &[(false, 0)],
        accepted: false,
    },
    Row {
        case: "one subnet of the own VPC on a network load balancer",
        load_balancer: 1,
        spelling: Spelling::Subnets,
        named: &[(true, 2)],
        accepted: true,
    },
];

#[test]
fn a_placement_spanning_two_vpcs_is_refused() {
    std::thread::Builder::new()
        .name("elbv2-subnet-vpc-membership".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                // The IO and TIME drivers, as the sibling suites whose services open a
                // loopback listener or wait on the clock enable them: a service this router
                // mounts may do either on a background task of the request it served, and a
                // runtime without those drivers panics inside tokio when it does.
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // The two VPCs every row runs against, created through EC2 itself so every id
            // below is one EC2 minted.
            let (own_vpc, own_subnets) = vpc_with_subnets(
                &router,
                &runtime,
                "10.104",
                &["us-west-2a", "us-west-2b", "us-west-2c"],
            );
            // The other VPC's subnets are in the zones NO subnet of the load balancer's
            // own VPC is named from below, so every mixed row still names one subnet per
            // Availability Zone.
            let (other_vpc, other_subnets) =
                vpc_with_subnets(&router, &runtime, "10.105", &["us-west-2c", "us-west-2b"]);
            assert_ne!(
                own_vpc, other_vpc,
                "the two VPCs must be distinct, got {own_vpc} and {other_vpc}"
            );
            // The refusal names the VPC the load balancer is in, taken from this service's
            // own state rather than from the request.
            let refusal = format!("All subnets must belong to the same VPC: '{own_vpc}'");

            // -----------------------------------------------------------------
            // A create whose subnets span two VPCs places nothing.
            // -----------------------------------------------------------------
            // (case, the subnets the create names, whether it is accepted)
            let create_rows: Vec<(&str, Vec<String>, bool)> = vec![
                (
                    "a create in two subnets of one VPC",
                    vec![own_subnets[0].clone(), own_subnets[1].clone()],
                    true,
                ),
                (
                    "a create mixing two VPCs",
                    vec![own_subnets[0].clone(), other_subnets[0].clone()],
                    false,
                ),
                (
                    "a create mixing two VPCs, the foreign subnet first",
                    vec![other_subnets[0].clone(), own_subnets[1].clone()],
                    false,
                ),
            ];
            for (index, (case, subnets, accepted)) in create_rows.iter().enumerate() {
                let name = format!("vpc-membership-{index}");
                let mut params = vec![
                    ("Name".to_string(), name.clone()),
                    ("Scheme".to_string(), "internal".to_string()),
                ];
                params.extend(placement(Spelling::Subnets, subnets));
                let answer = call(&router, &runtime, "CreateLoadBalancer", &params);
                if *accepted {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected the create to be accepted, got {answer:?}"
                    );
                } else {
                    assert_eq!(
                        (answer.status, answer.code()),
                        (400, INVALID_CONFIGURATION_REQUEST.to_string()),
                        "case {case:?}: expected 400 {INVALID_CONFIGURATION_REQUEST}, got \
                         {answer:?}"
                    );
                    // A refused create stores nothing: the name is still free.
                    let read = elbv2(
                        &router,
                        &runtime,
                        "DescribeLoadBalancers",
                        &[("Names.member.1", name.as_str())],
                    );
                    assert_eq!(
                        (read.status, read.code()),
                        (400, "LoadBalancerNotFound".to_string()),
                        "case {case:?}: a refused create must store no load balancer, got \
                         {read:?}"
                    );
                }
            }

            // -----------------------------------------------------------------
            // Nor does a replacement move a load balancer into another VPC.
            // -----------------------------------------------------------------
            let application = element(
                &call(
                    &router,
                    &runtime,
                    "DescribeLoadBalancers",
                    &[("Names.member.1".to_string(), "vpc-membership-0".to_string())],
                )
                .text,
                "LoadBalancerArn",
            );
            let mut network_params = vec![
                ("Name".to_string(), "vpc-membership-nlb".to_string()),
                ("Type".to_string(), "network".to_string()),
                ("Scheme".to_string(), "internal".to_string()),
            ];
            network_params.extend(placement(
                Spelling::Subnets,
                &[own_subnets[0].clone()],
            ));
            let created = call(&router, &runtime, "CreateLoadBalancer", &network_params);
            assert_eq!(
                created.status, 200,
                "setup: the network load balancer must be created, got {created:?}"
            );
            let network = element(&created.text, "LoadBalancerArn");
            let arns = [application, network];

            for row in ROWS {
                let named: Vec<String> = row
                    .named
                    .iter()
                    .map(|(own, index)| {
                        if *own {
                            own_subnets[*index].clone()
                        } else {
                            other_subnets[*index].clone()
                        }
                    })
                    .collect();
                let arn = &arns[row.load_balancer];
                let before = placed_subnets(&router, &runtime, arn);
                let mut params = vec![("LoadBalancerArn".to_string(), arn.clone())];
                params.extend(placement(row.spelling, &named));
                let answer = call(&router, &runtime, "SetSubnets", &params);
                let mut expected = named.clone();
                expected.sort();
                if row.accepted {
                    assert_eq!(
                        answer.status, 200,
                        "case {:?}: expected the replacement to be accepted, got {answer:?}",
                        row.case
                    );
                } else {
                    assert_eq!(
                        (answer.status, answer.code(), answer.message()),
                        (
                            400,
                            INVALID_CONFIGURATION_REQUEST.to_string(),
                            refusal.clone()
                        ),
                        "case {:?}: expected 400 {INVALID_CONFIGURATION_REQUEST} {refusal:?}, \
                         got {answer:?}",
                        row.case
                    );
                    // A refused replacement leaves the load balancer where it was.
                    expected = before.clone();
                }
                let after = placed_subnets(&router, &runtime, arn);
                assert_eq!(
                    after, expected,
                    "case {:?}: expected the load balancer to be placed in {expected:?}, got \
                     {after:?}",
                    row.case
                );
            }
        })
        .expect("the suite thread starts")
        .join()
        .expect("the suite thread completes");
}
