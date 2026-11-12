// SPDX-License-Identifier: CC0-1.0

//! The two Elastic Load Balancing refusals that are decided on what EC2 reports, and the
//! CODE each is answered under — driven with both services on one engine.
//!
//! * AN IPv6 ADDRESS TYPE NEEDS IPv6 SUBNETS. Every `IpAddressType` but `ipv4` gives the
//!   load balancer IPv6 addresses, and those come out of the IPv6 range of each subnet it is
//!   placed in, so a placement asking for one needs subnets that have an IPv6 CIDR block.
//!   The load balancer's SCHEME decides nothing — an internal load balancer whose subnets
//!   carry IPv6 ranges is dualstack like any other — and the refusal is a value the request
//!   has to change, so it is answered as `ValidationError` (HTTP 400) in the words the
//!   service words that rule, on the create, on the replacement and on `SetIpAddressType`.
//! * A SECURITY GROUP THAT CANNOT GOVERN THIS LOAD BALANCER'S TRAFFIC. A group this account
//!   holds none of, and a group of another VPC, are both configurations the named resources
//!   cannot hold: the modelled `InvalidConfigurationRequest`. That check runs BEFORE the
//!   name is judged for uniqueness, so a caller whose request is wrong in both ways is told
//!   about the groups rather than sent off to rename.
//!
//! Which ranges a subnet holds, and whether a security group exists and where, are EC2's to
//! say, so every row here runs against a VPC created through EC2 itself. The halves that
//! need no EC2 — the rule tables themselves, the attribute-key rule and the subnet-count
//! rule — are in the ELBv2 crate's `tests/all/input_rejection_classes.rs`.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread. The request plumbing is
//! `elbv2_network_resolution.rs`'s, reused rather than written twice.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_network_resolution::{ec2_ok, elbv2, Answer, BOOT_STACK_SIZE};
use crate::query_wire::{element, elements};

/// The code Elastic Load Balancing answers a value it cannot accept with, for a rule its
/// model documents in prose and declares no error shape for.
const VALIDATION_ERROR: &str = "ValidationError";

/// The awsQuery code of `InvalidConfigurationRequestException`, which the model binds to
/// HTTP 400 (`smithy_query(query="operation",operation="CreateLoadBalancer")`).
const INVALID_CONFIGURATION: &str = "InvalidConfigurationRequest";

/// The awsQuery code of `DuplicateLoadBalancerNameException`.
const DUPLICATE_NAME: &str = "DuplicateLoadBalancerName";

/// What the service states when a placement asking for IPv6 addresses names subnets that
/// have none.
const IPV6_CIDR_REQUIRED: &str = "You must specify subnets with an associated IPv6 CIDR block.";

/// What it states when the security groups a request names cannot govern this load
/// balancer's traffic.
const INVALID_SECURITY_GROUPS: &str = "One or more security groups are invalid";

/// A well-formed security group id this account holds no group for: EC2 mints ids of 17
/// hexadecimal digits, and nothing in this suite creates this one.
const ABSENT_GROUP: &str = "sg-0fedcba987654321f";

/// The network every row runs against: one VPC with two subnets — IPv4 only, which is what
/// `CreateSubnet` makes — and a security group, beside a second VPC with its own group.
struct Network {
    subnets: Vec<String>,
    security_group: String,
    other_security_group: String,
}

/// Create that network through EC2 itself, so every id below is one EC2 minted.
fn seed(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) -> Network {
    let vpc = element(
        &ec2_ok(router, runtime, "CreateVpc", &[("CidrBlock", "10.60.0.0/16")]),
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (cidr, zone) in [("10.60.1.0/24", "us-west-2a"), ("10.60.2.0/24", "us-west-2b")] {
        subnets.push(element(
            &ec2_ok(
                router,
                runtime,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", cidr),
                    ("AvailabilityZone", zone),
                ],
            ),
            "subnetId",
        ));
    }
    let security_group = element(
        &ec2_ok(
            router,
            runtime,
            "CreateSecurityGroup",
            &[
                ("GroupName", "elbv2-classes-own"),
                ("GroupDescription", "the load balancers own VPC"),
                ("VpcId", vpc.as_str()),
            ],
        ),
        "groupId",
    );
    let other_vpc = element(
        &ec2_ok(router, runtime, "CreateVpc", &[("CidrBlock", "10.61.0.0/16")]),
        "vpcId",
    );
    let other_security_group = element(
        &ec2_ok(
            router,
            runtime,
            "CreateSecurityGroup",
            &[
                ("GroupName", "elbv2-classes-foreign"),
                ("GroupDescription", "another VPC"),
                ("VpcId", other_vpc.as_str()),
            ],
        ),
        "groupId",
    );
    Network {
        subnets,
        security_group,
        other_security_group,
    }
}

/// The refusal an answer carries: its status, code and message, so one assertion names all
/// three.
fn refusal(answer: &Answer) -> (u16, String, String) {
    (answer.status, answer.code(), answer.message())
}

/// One row of the address-type table: the case, the scheme and type the load balancer is
/// created with, the address type asked for, and the code the request is refused under
/// (empty for one that is served).
struct AddressTypeRow {
    case: &'static str,
    scheme: &'static str,
    lb_type: &'static str,
    ip_address_type: &'static str,
    code: &'static str,
}

/// Every address type against both schemes and the families that carry it. The subnets of
/// the seeded VPC hold IPv4 ranges only, so every row asking for IPv6 addresses is refused
/// whatever its scheme, and `ipv4` is served whatever its scheme.
const ADDRESS_TYPE_ROWS: &[AddressTypeRow] = &[
    AddressTypeRow {
        case: "ipv4 on an internal application load balancer",
        scheme: "internal",
        lb_type: "application",
        ip_address_type: "ipv4",
        code: "",
    },
    AddressTypeRow {
        case: "ipv4 on an internet-facing application load balancer",
        scheme: "internet-facing",
        lb_type: "application",
        ip_address_type: "ipv4",
        code: "",
    },
    AddressTypeRow {
        case: "dualstack on an internal application load balancer",
        scheme: "internal",
        lb_type: "application",
        ip_address_type: "dualstack",
        code: VALIDATION_ERROR,
    },
    AddressTypeRow {
        case: "dualstack on an internet-facing application load balancer",
        scheme: "internet-facing",
        lb_type: "application",
        ip_address_type: "dualstack",
        code: VALIDATION_ERROR,
    },
    AddressTypeRow {
        case: "dualstack on an internal network load balancer",
        scheme: "internal",
        lb_type: "network",
        ip_address_type: "dualstack",
        code: VALIDATION_ERROR,
    },
    AddressTypeRow {
        case: "the application-only type on an application load balancer",
        scheme: "internet-facing",
        lb_type: "application",
        ip_address_type: "dualstack-without-public-ipv4",
        code: VALIDATION_ERROR,
    },
];

#[test]
fn an_ipv6_address_type_is_refused_until_the_subnets_carry_an_ipv6_range() {
    std::thread::Builder::new()
        .name("elbv2-dualstack-and-group-classes".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                // See the sibling `elbv2_subnet_vpc_membership` suite: the IO and TIME
                // drivers a service's background work needs, without which tokio panics.
                .enable_all()
                .build()
                .expect("a current-thread runtime");
            let network = seed(&router, &runtime);

            // -----------------------------------------------------------------
            // The create.
            // -----------------------------------------------------------------
            for (index, row) in ADDRESS_TYPE_ROWS.iter().enumerate() {
                let name = format!("ipv6-rule-{index}");
                let answer = elbv2(
                    &router,
                    &runtime,
                    "CreateLoadBalancer",
                    &[
                        ("Name", name.as_str()),
                        ("Scheme", row.scheme),
                        ("Type", row.lb_type),
                        ("IpAddressType", row.ip_address_type),
                        ("Subnets.member.1", network.subnets[0].as_str()),
                        ("Subnets.member.2", network.subnets[1].as_str()),
                    ],
                );
                if row.code.is_empty() {
                    assert_eq!(
                        answer.status, 200,
                        "case {:?}: expected the create to be accepted, got {answer:?}",
                        row.case
                    );
                    continue;
                }
                assert_eq!(
                    refusal(&answer),
                    (
                        400,
                        row.code.to_string(),
                        IPV6_CIDR_REQUIRED.to_string()
                    ),
                    "case {:?}: expected 400 {} {IPV6_CIDR_REQUIRED:?}, got {answer:?}",
                    row.case,
                    row.code
                );
                // A refused create stores nothing.
                let read = elbv2(
                    &router,
                    &runtime,
                    "DescribeLoadBalancers",
                    &[("Names.member.1", name.as_str())],
                );
                assert_eq!(
                    (read.status, read.code()),
                    (400, "LoadBalancerNotFound".to_string()),
                    "case {:?}: a refused create must store no load balancer, got {read:?}",
                    row.case
                );
            }

            // -----------------------------------------------------------------
            // `SetIpAddressType`, and `SetSubnets` asking for the type alongside
            // the placement: the same rule on a load balancer that exists.
            // -----------------------------------------------------------------
            let created = elbv2(
                &router,
                &runtime,
                "CreateLoadBalancer",
                &[
                    ("Name", "ipv6-rule-set"),
                    ("Scheme", "internal"),
                    ("Subnets.member.1", network.subnets[0].as_str()),
                    ("Subnets.member.2", network.subnets[1].as_str()),
                ],
            );
            assert_eq!(
                created.status, 200,
                "setup: the load balancer to reconfigure must be created, got {created:?}"
            );
            let arn = element(&created.text, "LoadBalancerArn");

            // (case, the type asked for, the code it is refused under — empty for one that
            // is served)
            let set_rows: [(&str, &str, &str); 4] = [
                ("ipv4, which the subnets serve", "ipv4", ""),
                ("dualstack", "dualstack", VALIDATION_ERROR),
                (
                    "the application-only type",
                    "dualstack-without-public-ipv4",
                    VALIDATION_ERROR,
                ),
                ("ipv4 again, after the refusals", "ipv4", ""),
            ];
            for (case, ip_address_type, code) in set_rows {
                let answer = elbv2(
                    &router,
                    &runtime,
                    "SetIpAddressType",
                    &[
                        ("LoadBalancerArn", arn.as_str()),
                        ("IpAddressType", ip_address_type),
                    ],
                );
                if code.is_empty() {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected 200, got {answer:?}"
                    );
                } else {
                    assert_eq!(
                        refusal(&answer),
                        (400, code.to_string(), IPV6_CIDR_REQUIRED.to_string()),
                        "case {case:?}: expected 400 {code} {IPV6_CIDR_REQUIRED:?}, got \
                         {answer:?}"
                    );
                }
                // Whatever the row, the load balancer still reports the only type its
                // subnets can serve: a refused call changed nothing.
                let read = elbv2(
                    &router,
                    &runtime,
                    "DescribeLoadBalancers",
                    &[("LoadBalancerArns.member.1", arn.as_str())],
                );
                assert_eq!(
                    elements(&read.text, "IpAddressType"),
                    vec!["ipv4".to_string()],
                    "case {case:?}: expected the stored type to be ipv4, got {read:?}"
                );
            }

            let replaced = elbv2(
                &router,
                &runtime,
                "SetSubnets",
                &[
                    ("LoadBalancerArn", arn.as_str()),
                    ("Subnets.member.1", network.subnets[0].as_str()),
                    ("Subnets.member.2", network.subnets[1].as_str()),
                    ("IpAddressType", "dualstack"),
                ],
            );
            assert_eq!(
                refusal(&replaced),
                (
                    400,
                    VALIDATION_ERROR.to_string(),
                    IPV6_CIDR_REQUIRED.to_string()
                ),
                "case \"a replacement asking for dualstack\": expected 400 \
                 {VALIDATION_ERROR} {IPV6_CIDR_REQUIRED:?}, got {replaced:?}"
            );

            // -----------------------------------------------------------------
            // The security groups, judged before the name.
            // -----------------------------------------------------------------
            let taken = "group-before-name";
            let first = elbv2(
                &router,
                &runtime,
                "CreateLoadBalancer",
                &[
                    ("Name", taken),
                    ("Scheme", "internal"),
                    ("Subnets.member.1", network.subnets[0].as_str()),
                    ("Subnets.member.2", network.subnets[1].as_str()),
                    ("SecurityGroups.member.1", network.security_group.as_str()),
                ],
            );
            assert_eq!(
                first.status, 200,
                "setup: the load balancer whose name the rows re-use must be created, got \
                 {first:?}"
            );

            // (case, the groups the request names, the code it earns, the message it
            // states) — every row re-uses the name above with a different scheme, so the
            // name alone would be refused as a duplicate: WHICH refusal the request earns
            // is what is under test.
            let group_rows: Vec<(&str, Vec<&str>, &str, &str)> = vec![
                (
                    "an id the account holds no group for",
                    vec![ABSENT_GROUP],
                    INVALID_CONFIGURATION,
                    INVALID_SECURITY_GROUPS,
                ),
                (
                    "a group of another VPC",
                    vec![network.other_security_group.as_str()],
                    INVALID_CONFIGURATION,
                    INVALID_SECURITY_GROUPS,
                ),
                (
                    "a group of each VPC",
                    vec![
                        network.security_group.as_str(),
                        network.other_security_group.as_str(),
                    ],
                    INVALID_CONFIGURATION,
                    INVALID_SECURITY_GROUPS,
                ),
                (
                    "the load balancer's own group, leaving only the name to fault",
                    vec![network.security_group.as_str()],
                    DUPLICATE_NAME,
                    "A load balancer with the same name 'group-before-name' exists, but \
                     with different settings",
                ),
            ];
            for (case, groups, code, message) in group_rows {
                let mut params = vec![
                    ("Name", taken),
                    ("Scheme", "internet-facing"),
                    ("Subnets.member.1", network.subnets[0].as_str()),
                    ("Subnets.member.2", network.subnets[1].as_str()),
                ];
                let keys = [
                    "SecurityGroups.member.1",
                    "SecurityGroups.member.2",
                ];
                for (key, id) in keys.iter().zip(groups.iter()) {
                    params.push((key, id));
                }
                let answer = elbv2(&router, &runtime, "CreateLoadBalancer", &params);
                assert_eq!(
                    refusal(&answer),
                    (400, code.to_string(), message.to_string()),
                    "case {case:?}: expected 400 {code} {message:?}, got {answer:?}"
                );
            }

            // And a fresh name with a group the account does not hold is refused the same
            // way, and creates nothing: the rule is about the groups, not about the name.
            let fresh = elbv2(
                &router,
                &runtime,
                "CreateLoadBalancer",
                &[
                    ("Name", "group-fresh-name"),
                    ("Scheme", "internal"),
                    ("Subnets.member.1", network.subnets[0].as_str()),
                    ("Subnets.member.2", network.subnets[1].as_str()),
                    ("SecurityGroups.member.1", ABSENT_GROUP),
                ],
            );
            assert_eq!(
                refusal(&fresh),
                (
                    400,
                    INVALID_CONFIGURATION.to_string(),
                    INVALID_SECURITY_GROUPS.to_string()
                ),
                "case \"a fresh name with a group the account does not hold\": expected 400 \
                 {INVALID_CONFIGURATION} {INVALID_SECURITY_GROUPS:?}, got {fresh:?}"
            );
            let read = elbv2(
                &router,
                &runtime,
                "DescribeLoadBalancers",
                &[("Names.member.1", "group-fresh-name")],
            );
            assert_eq!(
                (read.status, read.code()),
                (400, "LoadBalancerNotFound".to_string()),
                "case \"a fresh name with a group the account does not hold\": a refused \
                 create must store no load balancer, got {read:?}"
            );
        })
        .expect("the suite thread starts")
        .join()
        .expect("the suite thread completes");
}
