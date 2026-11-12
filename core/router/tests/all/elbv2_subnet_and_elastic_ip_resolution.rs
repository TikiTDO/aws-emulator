// SPDX-License-Identifier: CC0-1.0

//! A load balancer is placed in EC2's subnets and pinned to EC2's Elastic IP addresses, so
//! Elastic Load Balancing refuses a placement EC2 itself refuses.
//!
//! `CreateLoadBalancer` and `SetSubnets` both take `Subnets`/`SubnetMappings`, and neither
//! owns what those name: a subnet and an Elastic IP allocation are EC2's resources. So each
//! resolves them against EC2 through the in-process callback and answers the modelled
//! refusal — `SubnetNotFound` for an id the account holds no subnet for,
//! `AllocationIdNotFound` for one it holds no Elastic IP for — stating as its MESSAGE the
//! error CODE EC2 answered, which is this API's own wording for the class, rather than
//! placing a load balancer in a subnet that does not exist.
//!
//! That needs both services on one engine, which is why this suite lives here beside
//! `elbv2_network_resolution.rs`, whose request plumbing it reuses; the rules that need
//! only the request are tabled in the ELBv2 crate's `tests/all/load_balancer_input_rules.rs`.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::elbv2_network_resolution::{ec2_ok, elbv2, Answer, BOOT_STACK_SIZE};
use crate::query_wire::{element, opt_element};

/// The EC2 code for an id the account holds no subnet for, which is what Elastic Load
/// Balancing names in the refusal it reports when a placement names one.
const SUBNET_NOT_FOUND_CODE: &str = "InvalidSubnetID.NotFound";

/// The EC2 code for an id the account holds no Elastic IP allocation for.
const ALLOCATION_NOT_FOUND_CODE: &str = "InvalidAllocationID.NotFound";

/// The EC2 code for a value that is not an allocation id at all.
const ALLOCATION_MALFORMED_CODE: &str = "InvalidAllocationID.Malformed";

/// An id of the right form that names no subnet of the account.
const ABSENT_SUBNET: &str = "subnet-00000000000000000";

/// An id of the right form that names no Elastic IP allocation of the account.
const ABSENT_ALLOCATION: &str = "eipalloc-00000000000000000";

/// Assert one refusal Elastic Load Balancing answered for an EC2-owned resource it could
/// not resolve: the status, the modelled awsQuery code, and a message that is the error
/// CODE EC2 answered and nothing else — neither EC2's own sentence, nor the service and
/// status it came from, nor the id of the call that was made.
fn assert_refusal_states_the_inner_code(case: &str, answer: &Answer, code: &str, inner: &str) {
    assert_eq!(
        (answer.status, answer.code()),
        (400, code.to_string()),
        "case {case:?}: expected 400 {code}, got {answer:?}"
    );
    assert_eq!(
        answer.message(),
        inner,
        "case {case:?}: expected the message {inner:?}, got {:?}",
        answer.message()
    );
}

/// The refusal EC2 answered for an id it would not resolve: the error code it came under,
/// which is what the refusing operation states as its message.
struct InnerRefusal {
    code: &'static str,
}

/// One row of the Elastic IP table: the case, the allocation id the mapping pins, and the
/// refusal EC2 answers for it — `None` where EC2 holds the allocation and the create is
/// accepted.
struct AllocationRow {
    case: &'static str,
    pinned: String,
    refused: Option<InnerRefusal>,
}

#[test]
fn a_placement_naming_a_resource_ec2_does_not_hold_is_refused() {
    std::thread::Builder::new()
        .name("elbv2-subnet-and-elastic-ip-resolution".to_string())
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

            // The network every case runs against, created through EC2 itself so every id
            // below is one EC2 minted rather than one this suite made up.
            let vpc = element(
                &ec2_ok(
                    &router,
                    &runtime,
                    "CreateVpc",
                    &[("CidrBlock", "10.70.0.0/16")],
                ),
                "vpcId",
            );
            let mut subnets: Vec<String> = Vec::new();
            for (cidr, zone) in [("10.70.1.0/24", "us-west-2a"), ("10.70.2.0/24", "us-west-2b")] {
                subnets.push(element(
                    &ec2_ok(
                        &router,
                        &runtime,
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
            let allocation = element(
                &ec2_ok(&router, &runtime, "AllocateAddress", &[("Domain", "vpc")]),
                "allocationId",
            );

            // -----------------------------------------------------------------
            // The subnets a create names must be subnets EC2 holds.
            // -----------------------------------------------------------------
            // (case, the two subnet ids the request names, the id EC2 refuses the read
            // for — `None` when it resolves every one of them)
            let subnet_rows: Vec<(&str, [&str; 2], Option<&str>)> = vec![
                (
                    "two subnets EC2 holds",
                    [subnets[0].as_str(), subnets[1].as_str()],
                    None,
                ),
                (
                    "an id EC2 holds no subnet for",
                    [subnets[0].as_str(), ABSENT_SUBNET],
                    Some(ABSENT_SUBNET),
                ),
                (
                    "a value that is not a subnet id at all",
                    [subnets[0].as_str(), "not-a-subnet"],
                    Some("not-a-subnet"),
                ),
                (
                    "two ids EC2 holds no subnet for",
                    [ABSENT_SUBNET, "subnet-00000000000000001"],
                    // EC2 refuses the whole read on the FIRST id it cannot resolve, so
                    // that is the one its sentence names.
                    Some(ABSENT_SUBNET),
                ),
            ];
            for (index, (case, named, unresolved)) in subnet_rows.iter().enumerate() {
                let name = format!("subnet-resolution-{index}");
                let answer = elbv2(
                    &router,
                    &runtime,
                    "CreateLoadBalancer",
                    &[
                        ("Name", name.as_str()),
                        ("Scheme", "internal"),
                        ("Subnets.member.1", named[0]),
                        ("Subnets.member.2", named[1]),
                    ],
                );
                if let Some(unresolved) = unresolved {
                    assert_refusal_states_the_inner_code(
                        &format!("{case}, EC2 refusing the read for {unresolved:?}"),
                        &answer,
                        "SubnetNotFound",
                        SUBNET_NOT_FOUND_CODE,
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
                        "case {case:?}: a refused create must store nothing, got {read:?}"
                    );
                } else {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected the create to be accepted, got {answer:?}"
                    );
                    // The VPC comes from the subnets EC2 resolved, so an accepted create
                    // really did resolve them.
                    assert_eq!(
                        opt_element(&answer.text, "VpcId").as_deref(),
                        Some(vpc.as_str()),
                        "case {case:?}: expected the VPC of its subnets, got {}",
                        answer.text
                    );
                }
            }

            // -----------------------------------------------------------------
            // So must the subnets that REPLACE them.
            // -----------------------------------------------------------------
            let placed = elbv2(
                &router,
                &runtime,
                "DescribeLoadBalancers",
                &[("Names.member.1", "subnet-resolution-0")],
            );
            let arn = element(&placed.text, "LoadBalancerArn");
            let reconfigured = elbv2(
                &router,
                &runtime,
                "SetSubnets",
                &[
                    ("LoadBalancerArn", arn.as_str()),
                    ("Subnets.member.1", subnets[0].as_str()),
                    ("Subnets.member.2", ABSENT_SUBNET),
                ],
            );
            assert_refusal_states_the_inner_code(
                "replacing a placement with an id EC2 holds no subnet for",
                &reconfigured,
                "SubnetNotFound",
                SUBNET_NOT_FOUND_CODE,
            );
            // And the load balancer is still in the subnets it was created in.
            let after = elbv2(
                &router,
                &runtime,
                "DescribeLoadBalancers",
                &[("LoadBalancerArns.member.1", arn.as_str())],
            );
            assert_eq!(
                opt_element(&after.text, "SubnetId").as_deref(),
                Some(subnets[0].as_str()),
                "case \"a refused SetSubnets\": the placement must be unchanged, got {}",
                after.text
            );

            // -----------------------------------------------------------------
            // An Elastic IP a mapping pins must be one EC2 allocated.
            // -----------------------------------------------------------------
            let allocation_rows: Vec<AllocationRow> = vec![
                AllocationRow {
                    case: "an allocation EC2 holds",
                    pinned: allocation.clone(),
                    refused: None,
                },
                AllocationRow {
                    case: "an id EC2 holds no allocation for",
                    pinned: ABSENT_ALLOCATION.to_string(),
                    refused: Some(InnerRefusal {
                        code: ALLOCATION_NOT_FOUND_CODE,
                    }),
                },
                AllocationRow {
                    case: "a value that is not an allocation id",
                    pinned: "not-an-eip".to_string(),
                    // EC2 tells a value that is no allocation id apart from one that names
                    // no allocation, and the refusal states the code it answered — so the
                    // two cases are distinguishable from the message alone.
                    refused: Some(InnerRefusal {
                        code: ALLOCATION_MALFORMED_CODE,
                    }),
                },
            ];
            for (index, row) in allocation_rows.iter().enumerate() {
                let case = row.case;
                let name = format!("eip-resolution-{index}");
                let answer = elbv2(
                    &router,
                    &runtime,
                    "CreateLoadBalancer",
                    &[
                        ("Name", name.as_str()),
                        ("Type", "network"),
                        ("Scheme", "internet-facing"),
                        ("SubnetMappings.member.1.SubnetId", subnets[0].as_str()),
                        ("SubnetMappings.member.1.AllocationId", row.pinned.as_str()),
                    ],
                );
                if let Some(refused) = &row.refused {
                    assert_refusal_states_the_inner_code(
                        case,
                        &answer,
                        "AllocationIdNotFound",
                        refused.code,
                    );
                    let read = elbv2(
                        &router,
                        &runtime,
                        "DescribeLoadBalancers",
                        &[("Names.member.1", name.as_str())],
                    );
                    assert_eq!(
                        (read.status, read.code()),
                        (400, "LoadBalancerNotFound".to_string()),
                        "case {case:?}: a refused create must store nothing, got {read:?}"
                    );
                } else {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected the create to be accepted, got {answer:?}"
                    );
                    // What the caller pinned is read back off the stored record.
                    let read = elbv2(
                        &router,
                        &runtime,
                        "DescribeLoadBalancers",
                        &[("Names.member.1", name.as_str())],
                    );
                    assert_eq!(
                        opt_element(&read.text, "AllocationId").as_deref(),
                        Some(row.pinned.as_str()),
                        "case {case:?}: expected the pinned allocation to be reported, got {}",
                        read.text
                    );
                }
            }
        })
        .expect("the suite thread starts")
        .join()
        .expect("the suite thread completes");
}
