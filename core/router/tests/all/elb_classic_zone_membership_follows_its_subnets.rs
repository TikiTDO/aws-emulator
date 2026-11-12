// SPDX-License-Identifier: CC0-1.0

//! A Classic load balancer placed in subnets is in the Availability Zones EC2 reports
//! those subnets are in — and in the zones of the subnets it is attached to NOW.
//!
//! `CreateLoadBalancer` takes `AvailabilityZones` and/or `Subnets`, and
//! `DescribeLoadBalancers` reports BOTH members for every load balancer. A subnet is not
//! Elastic Load Balancing's resource, so which zone (and which VPC) it is in is EC2's to
//! say: the placement is resolved against EC2 through the in-process callback, kept beside
//! the subnet that produced it, and followed by `AttachLoadBalancerToSubnets` and
//! `DetachLoadBalancerFromSubnets` — a load balancer created from one subnet reports that
//! subnet's zone, attaching a second one adds that subnet's zone, and detaching a subnet
//! takes its zone away again.
//!
//! Every value asserted here is one EC2 minted or reported, never one this suite made up,
//! and every assertion is on what a LATER `DescribeLoadBalancers` reports rather than on
//! what the write answered alone. The rules that need no EC2 — the conflict refusals and
//! the message each states — are tabled in the Classic crate's own
//! `tests/all/documented_rule_refusals.rs`.
//!
//! That needs both services on one engine, which is why this suite lives here beside
//! `elbv2_network_resolution.rs`, whose request plumbing it reuses. Assembling the whole
//! router costs stack (see `boot_serve.rs`), so it is built — and the requests driven — on
//! a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_network_resolution::{ec2_ok, form, send, Answer, BOOT_STACK_SIZE};
use crate::query_wire::{element, elements, opt_element};

/// The API version a Classic Elastic Load Balancing awsQuery envelope names.
const CLASSIC_VERSION: &str = "2012-06-01";

/// An id of the right form that names no subnet of the account.
const ABSENT_SUBNET: &str = "subnet-00000000000000090";

/// Drive one awsQuery action against the Classic model, on the signing name its front door
/// claims.
///
/// `pub(crate)` because the sibling suite `elb_classic_zone_enable_turns_on_the_vpc.rs`
/// drives the same model over the same wire: one copy of the Classic request plumbing, so
/// the two cannot drift into sending different envelopes.
pub(crate) fn classic(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    send(
        router,
        runtime,
        "elasticloadbalancing",
        form(action, CLASSIC_VERSION, params),
    )
}

/// The `<member>` texts of the list element `name`, in document order — empty when the
/// answer carries the list with no members at all.
///
/// `pub(crate)` for the reason [`classic`] is.
pub(crate) fn list(body: &str, name: &str) -> Vec<String> {
    opt_element(body, name)
        .map(|inner| elements(&inner, "member"))
        .unwrap_or_default()
}

/// One load balancer as `DescribeLoadBalancers` reports its placement.
#[derive(Debug, PartialEq, Eq)]
struct Placement {
    zones: Vec<String>,
    subnets: Vec<String>,
    vpc_id: Option<String>,
}

/// Which subnet operation one step of the attach/detach walk drives.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// `AttachLoadBalancerToSubnets`.
    Attach,
    /// `DetachLoadBalancerFromSubnets`.
    Detach,
}

/// One row of the create table: the case, the placement fields the request carries, the
/// zones the description must then report, the subnets it must report, and whether it
/// reports the VPC those subnets belong to.
type CreateRow<'a> = (
    &'a str,
    Vec<(&'a str, &'a str)>,
    Vec<&'a str>,
    Vec<&'a str>,
    bool,
);

/// One step of the attach/detach walk: the case, which operation it drives, the subnet it
/// names, the awsQuery code it must answer with (`None` for a success), the subnets the
/// load balancer then holds and the zones it is then in.
type StepRow<'a> = (
    &'a str,
    Step,
    &'a str,
    Option<&'a str>,
    Vec<&'a str>,
    Vec<&'a str>,
);

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
        zones: list(&described.text, "AvailabilityZones"),
        subnets: list(&described.text, "Subnets"),
        vpc_id: opt_element(&described.text, "VPCId"),
    }
}

/// The one listener every load balancer below is created with: the placement is what these
/// rows differ along, not the routing.
fn listener() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Listeners.member.1.Protocol", "HTTP"),
        ("Listeners.member.1.LoadBalancerPort", "80"),
        ("Listeners.member.1.InstancePort", "80"),
    ]
}

#[test]
fn a_classic_load_balancers_zones_follow_the_subnets_it_is_attached_to() {
    std::thread::Builder::new()
        .name("elb-classic-zone-membership".to_string())
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

            // The network every case runs against, created through EC2 itself: one VPC
            // with three subnets — two zones, and a SECOND subnet of the first zone, which
            // is what the one-subnet-per-zone rule is stated over.
            let vpc = element(
                &ec2_ok(
                    &router,
                    &runtime,
                    "CreateVpc",
                    &[("CidrBlock", "10.90.0.0/16")],
                ),
                "vpcId",
            );
            // `(subnet id, the zone EC2 reports it is in)`, in the order created.
            let mut subnets: Vec<(String, String)> = Vec::new();
            for (cidr, zone) in [
                ("10.90.1.0/24", "us-west-2a"),
                ("10.90.2.0/24", "us-west-2b"),
                ("10.90.3.0/24", "us-west-2a"),
            ] {
                let created = ec2_ok(
                    &router,
                    &runtime,
                    "CreateSubnet",
                    &[
                        ("VpcId", vpc.as_str()),
                        ("CidrBlock", cidr),
                        ("AvailabilityZone", zone),
                    ],
                );
                subnets.push((
                    element(&created, "subnetId"),
                    element(&created, "availabilityZone"),
                ));
            }
            let (first, first_zone) = (subnets[0].0.as_str(), subnets[0].1.as_str());
            let (second, second_zone) = (subnets[1].0.as_str(), subnets[1].1.as_str());
            let (third, third_zone) = (subnets[2].0.as_str(), subnets[2].1.as_str());

            // -----------------------------------------------------------------
            // A create reports the zones of the subnets it places the load balancer in.
            // -----------------------------------------------------------------
            // The rows differ along what the rule generalises over: how many subnets the
            // request names, in which order, which zones they are in, whether zones are
            // named directly instead of or beside them, and a subnet EC2 holds nothing
            // for — which contributes no zone rather than an invented one.
            let create_rows: Vec<CreateRow<'_>> = vec![
                (
                    "one subnet",
                    vec![("Subnets.member.1", first)],
                    vec![first_zone],
                    vec![first],
                    true,
                ),
                (
                    "two subnets in two zones",
                    vec![
                        ("Subnets.member.1", first),
                        ("Subnets.member.2", second),
                    ],
                    vec![first_zone, second_zone],
                    vec![first, second],
                    true,
                ),
                (
                    "the same two subnets named the other way round",
                    vec![
                        ("Subnets.member.1", second),
                        ("Subnets.member.2", first),
                    ],
                    vec![second_zone, first_zone],
                    vec![second, first],
                    true,
                ),
                (
                    "zones named directly, with no subnet at all",
                    vec![("AvailabilityZones.member.1", first_zone)],
                    vec![first_zone],
                    vec![],
                    false,
                ),
                (
                    "a zone named directly beside a subnet of another zone",
                    vec![
                        ("AvailabilityZones.member.1", second_zone),
                        ("Subnets.member.1", first),
                    ],
                    vec![second_zone, first_zone],
                    vec![first],
                    true,
                ),
            ];

            for (index, (case, placement, zones, placed_in, in_vpc)) in
                create_rows.into_iter().enumerate()
            {
                let name = format!("classic-placement-{index}");
                let mut params = vec![("LoadBalancerName", name.as_str())];
                params.extend(listener());
                params.extend(placement);
                let created = classic(&router, &runtime, "CreateLoadBalancer", &params);
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: the create must be accepted, got {created:?}"
                );

                let reported = placement_of(&router, &runtime, &name);
                let expected = Placement {
                    zones: zones.iter().map(|zone| (*zone).to_string()).collect(),
                    subnets: placed_in
                        .iter()
                        .map(|subnet| (*subnet).to_string())
                        .collect(),
                    vpc_id: in_vpc.then(|| vpc.clone()),
                };
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected {expected:?}, got {reported:?}"
                );
            }

            // A subnet is EC2's resource, so an id EC2 holds no subnet for names no
            // placement at all: the create is refused with the error the model declares for
            // it, and no record is left behind for the name it asked for.
            let refused = {
                let mut params = vec![("LoadBalancerName", "classic-absent-subnet")];
                params.extend(listener());
                params.push(("Subnets.member.1", ABSENT_SUBNET));
                classic(&router, &runtime, "CreateLoadBalancer", &params)
            };
            assert_eq!(
                (refused.status, refused.code().as_str()),
                (400, "SubnetNotFound"),
                "case \"an id EC2 holds no subnet for\": the create must be refused, got \
                 {refused:?}"
            );
            let described = classic(
                &router,
                &runtime,
                "DescribeLoadBalancers",
                &[("LoadBalancerNames.member.1", "classic-absent-subnet")],
            );
            assert_eq!(
                (described.status, described.code().as_str()),
                (400, "LoadBalancerNotFound"),
                "case \"an id EC2 holds no subnet for\": the refused create must leave no \
                 record, got {described:?}"
            );

            // -----------------------------------------------------------------
            // …and the zone set follows every later attach and detach.
            // -----------------------------------------------------------------
            let name = "classic-attach-detach";
            let mut params = vec![
                ("LoadBalancerName", name),
                ("Scheme", "internal"),
                ("Subnets.member.1", first),
            ];
            params.extend(listener());
            let created = classic(&router, &runtime, "CreateLoadBalancer", &params);
            assert_eq!(
                created.status, 200,
                "case \"the attach/detach fixture\": the create must be accepted, got \
                 {created:?}"
            );

            let steps: Vec<StepRow<'_>> = vec![
                (
                    "attaching a subnet of a zone the load balancer is not in yet",
                    Step::Attach,
                    second,
                    None,
                    vec![first, second],
                    vec![first_zone, second_zone],
                ),
                (
                    "attaching the same subnet again re-states the same placement",
                    Step::Attach,
                    second,
                    None,
                    vec![first, second],
                    vec![first_zone, second_zone],
                ),
                (
                    // "You can add only one subnet per Availability Zone" (`Subnets`' own
                    // documentation): the third subnet is in the first one's zone.
                    "attaching a second subnet of a zone it is already in",
                    Step::Attach,
                    third,
                    Some("InvalidConfigurationRequest"),
                    vec![first, second],
                    vec![first_zone, second_zone],
                ),
                (
                    "detaching a subnet takes the zone it put the load balancer in with it",
                    Step::Detach,
                    first,
                    None,
                    vec![second],
                    vec![second_zone],
                ),
                (
                    "detaching a subnet it is not attached to changes nothing",
                    Step::Detach,
                    first,
                    None,
                    vec![second],
                    vec![second_zone],
                ),
                (
                    "the zone is free again once its subnet is gone",
                    Step::Attach,
                    third,
                    None,
                    vec![second, third],
                    vec![second_zone, third_zone],
                ),
                (
                    "detaching the last subnets leaves the load balancer in no zone",
                    Step::Detach,
                    second,
                    None,
                    vec![third],
                    vec![third_zone],
                ),
                (
                    // A load balancer placed in subnets serves traffic from those subnets
                    // alone, so the detach that would leave it with none is refused and the
                    // last subnet stays attached.
                    "detaching the last subnet is refused",
                    Step::Detach,
                    third,
                    Some("InvalidConfigurationRequest"),
                    vec![third],
                    vec![third_zone],
                ),
            ];

            for (case, step, subnet, refusal, held, zones) in steps {
                let action = match step {
                    Step::Attach => "AttachLoadBalancerToSubnets",
                    Step::Detach => "DetachLoadBalancerFromSubnets",
                };
                let answered = classic(
                    &router,
                    &runtime,
                    action,
                    &[("LoadBalancerName", name), ("Subnets.member.1", subnet)],
                );
                match refusal {
                    None => {
                        assert_eq!(
                            answered.status, 200,
                            "case {case:?}: {action} must be accepted, got {answered:?}"
                        );
                        // The answer is the whole stored set, not just what this call
                        // changed.
                        let answered_subnets = list(&answered.text, "Subnets");
                        assert_eq!(
                            answered_subnets,
                            held.iter()
                                .map(|subnet| (*subnet).to_string())
                                .collect::<Vec<String>>(),
                            "case {case:?}: {action} must answer the stored subnet set, \
                             got {answered_subnets:?}"
                        );
                    }
                    Some(code) => assert_eq!(
                        (answered.status, answered.code().as_str()),
                        (409, code),
                        "case {case:?}: expected 409 {code}, got {answered:?}"
                    ),
                }

                let reported = placement_of(&router, &runtime, name);
                let expected = Placement {
                    zones: zones.iter().map(|zone| (*zone).to_string()).collect(),
                    subnets: held.iter().map(|subnet| (*subnet).to_string()).collect(),
                    // The VPC the load balancer was placed in is a fact of the record, so
                    // it survives the subnets coming and going.
                    vpc_id: Some(vpc.clone()),
                };
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected {expected:?}, got {reported:?}"
                );
            }

            // A subnet operation on a name the region holds no load balancer for is the
            // modelled not-found refusal, so neither write invents a record.
            for action in [
                "AttachLoadBalancerToSubnets",
                "DetachLoadBalancerFromSubnets",
            ] {
                let answered = classic(
                    &router,
                    &runtime,
                    action,
                    &[
                        ("LoadBalancerName", "no-such-classic-elb"),
                        ("Subnets.member.1", first),
                    ],
                );
                assert_eq!(
                    (answered.status, answered.code().as_str()),
                    (400, "LoadBalancerNotFound"),
                    "case {action:?}: a name nothing holds must be refused, got {answered:?}"
                );
            }
        })
        .expect("the suite thread starts")
        .join()
        .expect("the suite thread completes");
}
