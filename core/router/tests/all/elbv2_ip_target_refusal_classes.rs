// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal an `ip` target registration earns is decided by the ADDRESS, and each
//! refusal states that class's own constraint against the address the request carried.
//!
//! The rule under test (`RegisterTargets`, the `ip` branch of the ELBv2 crate's
//! `src/state/network.rs`): an address is placed against the target group's VPC as EC2
//! reports it — in a subnet of it, in its CIDR but in none of its subnets, or outside it —
//! and only an address the group may carry at all (`TargetType`: "the IP addresses from
//! the subnets of the virtual private cloud (VPC) for the target group, the RFC 1918 range
//! … and the RFC 6598 range … You can't specify publicly routable IP addresses") is then
//! asked for the Availability Zone that is to serve it
//! (`TargetDescription$AvailabilityZone`: "if the IP address is outside the VPC, this
//! parameter is required"). So the presence of `AvailabilityZone` never selects the class,
//! and a caller reading the refusal to decide what to do next is told the truth: the zone
//! is asked for exactly where supplying it is the remedy.
//!
//! Every address below is driven TWICE — once with no zone and once with `all` — so the
//! table asserts both the class each address earns and whether naming all zones is a
//! remedy for it. The VPC and its subnet are created through EC2, which owns them, so the
//! placement each row asserts is one EC2 reported; that is why this suite lives here rather
//! than in the ELBv2 crate, whose request-only rules are tabled in its own
//! `tests/all/target_validation_and_quotas.rs`. The per-clause table of the same rule,
//! driven without a wire, is `src/state/vpc_topology.rs` in that crate.
//!
//! The emulator, the request builder and the readers are `elbv2_network_resolution.rs`'s,
//! shared rather than copied: the two suites drive the same two services over the same wire,
//! and a second copy of the plumbing would let them drift into asserting different things.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

use crate::elbv2_network_resolution::{
    ec2_ok, register, target_group, target_zones, BOOT_STACK_SIZE,
};
use crate::query_wire::element;

/// The VPC this suite's target group belongs to, and the one subnet of it.
const VPC_CIDR: &str = "10.11.0.0/16";
const SUBNET_CIDR: &str = "10.11.1.0/24";
const SUBNET_ZONE: &str = "us-west-2a";

/// Where an address falls relative to the target group's VPC — which is what decides the
/// answer a registration of it earns.
#[derive(Debug, Clone, Copy)]
enum Class {
    /// In a subnet of the VPC: accepted, in that subnet's zone whatever the request said.
    InSubnet,
    /// In the VPC's CIDR but in none of its subnets: refused, with no zone remedy.
    InVpcNoSubnet,
    /// Outside the VPC, from a range an `ip` target may come from: the zone that is to
    /// serve it is what is missing, so naming all zones IS the remedy.
    OutsideVpcPrivate,
    /// From no range an `ip` target may come from: refused on its own terms, with no zone
    /// remedy.
    NotAllowed,
}

impl Class {
    /// What this class owes for `address` when the request names `zone`: `Ok(the zone the
    /// registration reports)` or `Err(the refusal message)`.
    fn expected(self, address: &str, zone: Option<&str>) -> Result<String, String> {
        match self {
            Class::InSubnet => Ok(SUBNET_ZONE.to_string()),
            Class::InVpcNoSubnet => Err(format!(
                "The IP address '{address}' is within the VPC, but is not within a VPC \
                 subnet so it can't be registered"
            )),
            Class::NotAllowed => Err(format!(
                "The IP address '{address}' is not allowed, as it must be an address from \
                 a VPC subnet, RFC1918, or RFC6598"
            )),
            Class::OutsideVpcPrivate => match zone {
                Some(zone) => Ok(zone.to_string()),
                None => Err(format!(
                    "You must specify a valid Availability Zone or select 'all' for all \
                     enabled Availability zones, since the IP address '{address}' is \
                     outside the VPC"
                )),
            },
        }
    }
}

#[test]
fn an_ip_targets_refusal_class_is_chosen_by_the_address() {
    std::thread::Builder::new()
        .name("elbv2-ip-target-refusal-classes".to_string())
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

            // The network, created through EC2 itself: one VPC with one subnet, so every
            // placement below is one EC2 reported rather than one this suite made up.
            let vpc = element(
                &ec2_ok(&router, &runtime, "CreateVpc", &[("CidrBlock", VPC_CIDR)]),
                "vpcId",
            );
            let created_subnet = ec2_ok(
                &router,
                &runtime,
                "CreateSubnet",
                &[
                    ("VpcId", vpc.as_str()),
                    ("CidrBlock", SUBNET_CIDR),
                    ("AvailabilityZone", SUBNET_ZONE),
                ],
            );
            assert_eq!(
                element(&created_subnet, "availabilityZone"),
                SUBNET_ZONE,
                "setup: EC2 must report the subnet in the zone it was created in: \
                 {created_subnet}"
            );
            let group = target_group(
                &router,
                &runtime,
                "ip-refusal-classes-tg",
                "ip",
                vpc.as_str(),
            );

            // One row per address, spanning the four placements: inside a subnet, inside
            // the VPC's CIDR outside every subnet, outside the VPC from each range an `ip`
            // target may come from (RFC 1918's three blocks and RFC 6598), and outside it
            // from none of them (the two ranges RFC 5737 reserves for documentation, a
            // public resolver's address, and each published range bound passed by one).
            let rows: [(&str, &str, Class); 13] = [
                ("the subnet's own addresses", "10.11.1.5", Class::InSubnet),
                (
                    "the first address of the subnet",
                    "10.11.1.0",
                    Class::InSubnet,
                ),
                (
                    "inside the VPC CIDR, past the subnet",
                    "10.11.9.9",
                    Class::InVpcNoSubnet,
                ),
                (
                    "inside the VPC CIDR, below the subnet",
                    "10.11.0.7",
                    Class::InVpcNoSubnet,
                ),
                (
                    "RFC 1918's 192.168/16, outside the VPC",
                    "192.168.5.5",
                    Class::OutsideVpcPrivate,
                ),
                (
                    "RFC 1918's 172.16/12, outside the VPC",
                    "172.16.4.4",
                    Class::OutsideVpcPrivate,
                ),
                (
                    "RFC 1918's 10/8, outside this VPC",
                    "10.90.1.1",
                    Class::OutsideVpcPrivate,
                ),
                (
                    "RFC 6598's 100.64/10",
                    "100.64.3.3",
                    Class::OutsideVpcPrivate,
                ),
                ("a public resolver's address", "8.8.8.8", Class::NotAllowed),
                (
                    "RFC 5737's first documentation range",
                    "192.0.2.10",
                    Class::NotAllowed,
                ),
                (
                    "RFC 5737's third documentation range",
                    "203.0.113.4",
                    Class::NotAllowed,
                ),
                (
                    "just below RFC 1918's 172.16/12",
                    "172.15.255.254",
                    Class::NotAllowed,
                ),
                (
                    "just above RFC 6598's 100.64/10",
                    "100.128.0.1",
                    Class::NotAllowed,
                ),
            ];

            // Every address twice: with no zone, and with the `all` that is the remedy for
            // exactly one of the classes. `held` is what the group must report after each
            // call, so a refusal is also shown to register nothing.
            let mut held: Vec<(String, String)> = Vec::new();
            for (case, address, class) in rows {
                for zone in [None, Some("all")] {
                    let answer = register(&router, &runtime, &group, address, zone);
                    match class.expected(address, zone) {
                        Ok(expected_zone) => {
                            assert_eq!(
                                answer.status, 200,
                                "case {case:?} ({address}, zone {zone:?}): expected the \
                                 registration to be accepted, got {answer:?}"
                            );
                            held.retain(|(id, _)| id != address);
                            held.push((address.to_string(), expected_zone));
                        }
                        Err(expected_message) => {
                            assert_eq!(
                                (answer.status, answer.code()),
                                (400, "ValidationError".to_string()),
                                "case {case:?} ({address}, zone {zone:?}): expected 400 \
                                 ValidationError, got {answer:?}"
                            );
                            assert_eq!(
                                answer.message(),
                                expected_message,
                                "case {case:?} ({address}, zone {zone:?}): expected the \
                                 refusal {expected_message:?}, got {:?}",
                                answer.message()
                            );
                        }
                    }
                    let mut reported = target_zones(&router, &runtime, &group);
                    reported.sort();
                    let mut want = held.clone();
                    want.sort();
                    assert_eq!(
                        reported, want,
                        "case {case:?} ({address}, zone {zone:?}): the group must hold \
                         exactly the accepted registrations"
                    );
                }
            }
        })
        .expect("a sized thread")
        .join()
        .expect("the suite's thread");
}
