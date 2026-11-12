// SPDX-License-Identifier: CC0-1.0

//! Whether `EnableAvailabilityZonesForLoadBalancer` applies to a Classic load balancer at
//! all turns on the VPC the ACCOUNT'S NETWORK puts it in.
//!
//! RULE, from the operation's own documentation: it "adds the specified Availability Zones
//! to the set of Availability Zones for the specified load balancer in EC2-Classic or a
//! default VPC. For load balancers in a non-default VPC, use AttachLoadBalancerToSubnets."
//! So a load balancer EC2 reports in a VPC of the account's OWN is placed by its subnets,
//! and the zone API is refused for it — HTTP 400 under the code this awsQuery service
//! answers a request it cannot accept with, stating that rule — while the call is a success
//! path for a load balancer in no VPC at all and for one in the account's default VPC. A
//! refusal writes nothing: the zones and subnets a later `DescribeLoadBalancers` reports
//! are the ones it reported before, and the same zone is reachable through
//! `AttachLoadBalancerToSubnets`.
//!
//! A VPC is EC2's resource, and which of them is the account's default is EC2's to say, so
//! this needs both services on one engine — which is why the suite lives here beside
//! `elb_classic_zone_membership_follows_its_subnets.rs`, whose Classic request plumbing it
//! reuses. The rules that need no EC2 (the stored set a zone-enable grows, its refusals,
//! and the silence nothing is refused on) are tabled in the Classic crate's own
//! `tests/all/zone_membership_by_name.rs`.
//!
//! Every value asserted here is one EC2 minted or reported, never one this suite made up.
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::elb_classic_zone_membership_follows_its_subnets::{classic, list};
use crate::elbv2_network_resolution::{ec2_ok, BOOT_STACK_SIZE};
use crate::query_wire::element;

/// The sentence the zone API refuses a load balancer of a non-default VPC with: fixed text
/// that states which load balancers the operation applies to, with no part of the request
/// in it.
const NON_DEFAULT_VPC_REFUSAL: &str =
    "You cannot enable Availability Zones for ELBs in a non default VPC";

/// The error code this service answers a request whose values it cannot accept with.
const VALIDATION_ERROR: &str = "ValidationError";

/// Two Availability Zones of the Region every request below is signed for
/// (`elbv2_network_resolution::REGION`): the account's default VPC holds a default subnet
/// in each zone of its Region, and the VPC this suite creates is given a subnet in each of
/// these two. Which zone each subnet is really in is read back off EC2's own answer.
const ZONES: [&str; 2] = ["us-west-2a", "us-west-2b"];

/// One load balancer's placement, as `DescribeLoadBalancers` reports it.
#[derive(Debug, PartialEq, Eq)]
struct Placement {
    zones: Vec<String>,
    subnets: Vec<String>,
}

/// One subnet of the account, as EC2 reports the facts this suite places on.
#[derive(Debug, Clone)]
struct Subnet {
    id: String,
    zone: String,
}

/// One row of the accepted table: the case, the load balancer it creates, the placement
/// the create carries, the zone the request enables, and the zones the load balancer is in
/// afterwards.
struct Accepted<'a> {
    case: &'a str,
    name: &'a str,
    placement: Vec<(&'a str, &'a str)>,
    enabled: &'a str,
    zones: Vec<String>,
}

/// One row of the refused table: the case, and the `AvailabilityZones` fields the request
/// carries.
struct Refused<'a> {
    case: &'a str,
    zones: Vec<(&'a str, &'a str)>,
}

/// The one listener every load balancer below is created with: the placement is what the
/// cases differ along, not the routing.
fn listener() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Listeners.member.1.Protocol", "HTTP"),
        ("Listeners.member.1.LoadBalancerPort", "80"),
        ("Listeners.member.1.InstancePort", "80"),
    ]
}

/// Read `name`'s placement back through `DescribeLoadBalancers`.
fn placement_of(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
) -> Placement {
    let described = classic(
        router,
        runtime,
        "DescribeLoadBalancers",
        &[("LoadBalancerNames.member.1", name)],
    );
    assert_eq!(
        described.status, 200,
        "case {name:?}: the load balancer must be describable, got {described:?}"
    );
    Placement {
        zones: sorted(list(&described.text, "AvailabilityZones")),
        subnets: sorted(list(&described.text, "Subnets")),
    }
}

/// Zone and subnet sets compare as SETS: which zones a load balancer is in is the fact
/// under test, and the order the two placement members are reported in is pinned by the
/// sibling suite.
fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

fn owned(values: &[&str]) -> Vec<String> {
    sorted(values.iter().map(|value| (*value).to_string()).collect())
}

/// Create a Classic load balancer named `name` with the placement in `placement`.
fn create(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    placement: &[(&str, &str)],
) {
    let mut params = vec![("LoadBalancerName", name)];
    params.extend(listener());
    params.extend_from_slice(placement);
    let created = classic(router, runtime, "CreateLoadBalancer", &params);
    assert_eq!(
        created.status, 200,
        "setup: the load balancer {name} must be created, got {created:?}"
    );
}

/// The subnet EC2 holds in `vpc_id` in `zone`, read through EC2's OWN filters so the
/// answer carries that one subnet and both facts on it are EC2's.
fn subnet_in_zone(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    vpc_id: &str,
    zone: &str,
) -> Subnet {
    let described = ec2_ok(
        router,
        runtime,
        "DescribeSubnets",
        &[
            ("Filter.1.Name", "vpc-id"),
            ("Filter.1.Value.1", vpc_id),
            ("Filter.2.Name", "availability-zone"),
            ("Filter.2.Value.1", zone),
        ],
    );
    Subnet {
        id: element(&described, "subnetId"),
        zone: element(&described, "availabilityZone"),
    }
}

/// A subnet of `vpc_id` in each of [`ZONES`], in that order.
fn subnets_of(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    vpc_id: &str,
) -> Vec<Subnet> {
    ZONES
        .iter()
        .map(|zone| subnet_in_zone(router, runtime, vpc_id, zone))
        .collect()
}

/// The `VpcId` of the VPC EC2 reports as the account's DEFAULT one, read through the
/// filter EC2 serves that question under — so it is EC2 that says which VPC it is, not
/// this suite.
fn default_vpc(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) -> String {
    let described = ec2_ok(
        router,
        runtime,
        "DescribeVpcs",
        &[("Filter.1.Name", "isDefault"), ("Filter.1.Value.1", "true")],
    );
    element(&described, "vpcId")
}

#[test]
fn a_zone_enable_applies_only_outside_a_non_default_vpc() {
    std::thread::Builder::new()
        .name("elb-classic-zone-enable".to_string())
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

            // A VPC of the account's own, created through EC2 itself, with a subnet in each
            // of two zones…
            let own_vpc = element(
                &ec2_ok(
                    &router,
                    &runtime,
                    "CreateVpc",
                    &[("CidrBlock", "10.91.0.0/16")],
                ),
                "vpcId",
            );
            for (cidr, zone) in [("10.91.1.0/24", ZONES[0]), ("10.91.2.0/24", ZONES[1])] {
                ec2_ok(
                    &router,
                    &runtime,
                    "CreateSubnet",
                    &[
                        ("VpcId", own_vpc.as_str()),
                        ("CidrBlock", cidr),
                        ("AvailabilityZone", zone),
                    ],
                );
            }
            let own_subnets = subnets_of(&router, &runtime, &own_vpc);
            // …and the default VPC the account has held since it existed, whose subnets EC2
            // reports rather than this suite creating them.
            let default_subnets = {
                let default_vpc = default_vpc(&router, &runtime);
                subnets_of(&router, &runtime, &default_vpc)
            };

            // -----------------------------------------------------------------
            // The success paths: EC2-Classic (no VPC at all) and the default VPC.
            // -----------------------------------------------------------------
            // (case, the name, the placement the create carries, the zone enabled, the
            // zones the load balancer is then in)
            let accepted: Vec<Accepted<'_>> = vec![
                Accepted {
                    case: "a load balancer in no VPC, placed by zone",
                    name: "zone-enable-classic",
                    placement: vec![(
                        "AvailabilityZones.member.1",
                        own_subnets[0].zone.as_str(),
                    )],
                    enabled: own_subnets[1].zone.as_str(),
                    zones: vec![own_subnets[0].zone.clone(), own_subnets[1].zone.clone()],
                },
                Accepted {
                    case: "a load balancer in the account's default VPC",
                    name: "zone-enable-default-vpc",
                    placement: vec![("Subnets.member.1", default_subnets[0].id.as_str())],
                    enabled: default_subnets[1].zone.as_str(),
                    zones: vec![
                        default_subnets[0].zone.clone(),
                        default_subnets[1].zone.clone(),
                    ],
                },
                Accepted {
                    case: "a zone the load balancer of the default VPC is already in",
                    name: "zone-enable-default-vpc-held",
                    placement: vec![("Subnets.member.1", default_subnets[0].id.as_str())],
                    enabled: default_subnets[0].zone.as_str(),
                    zones: vec![default_subnets[0].zone.clone()],
                },
            ];

            for Accepted {
                case,
                name,
                placement,
                enabled,
                zones,
            } in accepted
            {
                create(&router, &runtime, name, &placement);
                let answered = classic(
                    &router,
                    &runtime,
                    "EnableAvailabilityZonesForLoadBalancer",
                    &[
                        ("LoadBalancerName", name),
                        ("AvailabilityZones.member.1", enabled),
                    ],
                );
                assert_eq!(
                    answered.status, 200,
                    "case {case:?}: the zone-enable must be accepted, got {answered:?}"
                );
                let expected = sorted(zones);
                let answered_zones = sorted(list(&answered.text, "AvailabilityZones"));
                assert_eq!(
                    answered_zones, expected,
                    "case {case:?}: expected the answer to carry the stored set \
                     {expected:?}, got {answered_zones:?}"
                );
                let reported = placement_of(&router, &runtime, name);
                assert_eq!(
                    reported.zones, expected,
                    "case {case:?}: expected a later describe to report {expected:?}, got \
                     {reported:?}"
                );
            }

            // -----------------------------------------------------------------
            // The refusal: a load balancer EC2 reports in a VPC of the account's own.
            // -----------------------------------------------------------------
            let name = "zone-enable-own-vpc";
            create(
                &router,
                &runtime,
                name,
                &[("Scheme", "internal"), ("Subnets.member.1", own_subnets[0].id.as_str())],
            );
            let before = placement_of(&router, &runtime, name);
            assert_eq!(
                before,
                Placement {
                    zones: owned(&[own_subnets[0].zone.as_str()]),
                    subnets: owned(&[own_subnets[0].id.as_str()]),
                },
                "setup: the load balancer must start in its first subnet alone, got \
                 {before:?}"
            );

            // The rows differ along which zone the request names — one the load balancer is
            // not in, one it already is in, and two at once — because the refusal is about
            // the load balancer's NETWORK and not about the zones the request carries.
            let refused: Vec<Refused<'_>> = vec![
                Refused {
                    case: "a zone the load balancer is not in",
                    zones: vec![(
                        "AvailabilityZones.member.1",
                        own_subnets[1].zone.as_str(),
                    )],
                },
                Refused {
                    case: "a zone it is already in",
                    zones: vec![(
                        "AvailabilityZones.member.1",
                        own_subnets[0].zone.as_str(),
                    )],
                },
                Refused {
                    case: "two zones at once",
                    zones: vec![
                        ("AvailabilityZones.member.1", own_subnets[0].zone.as_str()),
                        ("AvailabilityZones.member.2", own_subnets[1].zone.as_str()),
                    ],
                },
            ];

            for Refused { case, zones } in refused {
                let mut params = vec![("LoadBalancerName", name)];
                params.extend(zones);
                let answered = classic(
                    &router,
                    &runtime,
                    "EnableAvailabilityZonesForLoadBalancer",
                    &params,
                );
                assert_eq!(
                    (answered.status, answered.code().as_str()),
                    (400, VALIDATION_ERROR),
                    "case {case:?}: expected 400 {VALIDATION_ERROR}, got {answered:?}"
                );
                let stated = answered.message();
                assert_eq!(
                    stated, NON_DEFAULT_VPC_REFUSAL,
                    "case {case:?}: expected the message {NON_DEFAULT_VPC_REFUSAL:?}, got \
                     {stated:?}"
                );
                let after = placement_of(&router, &runtime, name);
                assert_eq!(
                    after, before,
                    "case {case:?}: the refusal must write nothing, expected {before:?}, \
                     got {after:?}"
                );
            }

            // The operation the documentation points such a load balancer at DOES reach the
            // same zone: the refusal is "use the other API", not "this zone is unreachable".
            let attached = classic(
                &router,
                &runtime,
                "AttachLoadBalancerToSubnets",
                &[
                    ("LoadBalancerName", name),
                    ("Subnets.member.1", own_subnets[1].id.as_str()),
                ],
            );
            assert_eq!(
                attached.status, 200,
                "case \"the VPC fallback path\": the attach must be accepted, got {attached:?}"
            );
            let attached_subnets = sorted(list(&attached.text, "Subnets"));
            let both_subnets = owned(&[own_subnets[0].id.as_str(), own_subnets[1].id.as_str()]);
            assert_eq!(
                attached_subnets, both_subnets,
                "case \"the VPC fallback path\": the attach must answer the stored subnet \
                 set {both_subnets:?}, got {attached_subnets:?}"
            );
            let after_attach = placement_of(&router, &runtime, name);
            assert_eq!(
                after_attach,
                Placement {
                    zones: owned(&[
                        own_subnets[0].zone.as_str(),
                        own_subnets[1].zone.as_str()
                    ]),
                    subnets: both_subnets,
                },
                "case \"the VPC fallback path\": the zone the zone-enable refused must be \
                 reachable through the attach, got {after_attach:?}"
            );
        })
        .expect("the suite thread starts")
        .join()
        .expect("the suite thread completes");
}
