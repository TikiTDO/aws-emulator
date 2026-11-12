// SPDX-License-Identifier: CC0-1.0

//! Elastic Load Balancing v2 resolves the ACCOUNT'S NETWORK against EC2, which owns it,
//! and answers from what EC2 reported.
//!
//! A load balancer is placed in subnets, a target group belongs to a VPC and an `ip`
//! target is an address inside one — none of them a resource of ELBv2 — so each is
//! resolved through the in-process callback. That needs both services on one engine, which
//! is why this suite lives here rather than in the ELBv2 crate (the rules that need only
//! the request are tabled in that crate's
//! `tests/all/target_validation_and_quotas.rs`).
//!
//! Four rules are under test:
//!
//! * THE LOAD BALANCER'S VPC AND ZONES. `DescribeLoadBalancers` reports the VpcId the
//!   load balancer's subnets belong to, and each zone it is attached to is the zone of the
//!   subnet that put it there — both read off EC2's subnets rather than invented.
//! * ITS SECURITY GROUPS. A security group governs traffic to network interfaces in those
//!   subnets, so a group of another VPC can never apply: `CreateLoadBalancer` refuses it
//!   with `InvalidConfigurationRequest` (HTTP 400) and creates nothing — through either
//!   name a client addresses this model by ([`ELBV2_DOORS`]), since what the placement is
//!   judged on is the network and not the front door the request arrived at.
//! * AN `ip` TARGET. `RegisterTargets` accepts an address in a subnet of the target
//!   group's VPC — and reports that subnet's zone for it, whatever the request said — and
//!   refuses an address in the VPC's CIDR that no subnet holds, a publicly routable
//!   address, and an address outside the VPC with no Availability Zone to serve it
//!   (`ValidationError`, HTTP 400). An address outside the VPC from a private range IS
//!   accepted once the request names the zone.
//! * AN `instance` TARGET. The modelled `InvalidTarget` is documented "the specified
//!   target does not exist, is not in the same VPC as the target group": an instance EC2
//!   holds in another VPC EXISTS and cannot be registered, so it earns that refusal, while
//!   the instance EC2 holds in the group's own VPC is accepted. A value that is not WRITTEN
//!   as an instance id, and an id EC2 holds no instance for, each name no target of any
//!   account and are refused as malformed input (`ValidationError`).
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
///
/// This constant and the request plumbing below are `pub(crate)` because the sibling suite
/// `elbv2_ip_target_refusal_classes.rs` drives the same two services over the same wire:
/// one copy, so the two cannot drift into asserting different things.
pub(crate) const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
pub(crate) const REGION: &str = "us-west-2";

/// The EC2 API version a client sends in an ec2Query body.
pub(crate) const EC2_API_VERSION: &str = "2016-11-15";

/// The API version an awsQuery request naming the ELBv2 model carries.
pub(crate) const ELB_VERSION: &str = "2015-12-01";

/// The two names a client addresses the ELBv2 model by: this emulator's own route key for
/// it, and the shared signing name AWS publishes both Elastic Load Balancing APIs under,
/// which is the one every AWS SDK ELBv2 client signs for. A request carrying the ELBv2 API
/// version is served out of this model through either.
const ELBV2_DOORS: [&str; 2] = ["elasticloadbalancingv2", "elasticloadbalancing"];

/// An AMI id a launch names; `RunInstances` only has to accept it, and this suite asserts
/// nothing about the image.
const AMI: &str = "ami-0123456789abcdef0";

/// One answer, reduced to what these suites branch on.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) text: String,
}

impl Answer {
    /// The awsQuery / ec2Query error code of a refusal, or an empty string.
    pub(crate) fn code(&self) -> String {
        opt_element(&self.text, "Code").unwrap_or_default()
    }

    /// The refusal message, with the serializer's escaping undone.
    pub(crate) fn message(&self) -> String {
        elements(&self.text, "Message").first().cloned().unwrap_or_default()
    }
}

/// Send one form-encoded request to the service `signing_name` addresses.
pub(crate) fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    body: String,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
                 /aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    Answer {
        status: response.status().as_u16(),
        text: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// One form body: the action, the version, and the parameters.
pub(crate) fn form(action: &str, version: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    body
}

/// Drive one ec2Query action.
pub(crate) fn ec2(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    send(router, runtime, "ec2", form(action, EC2_API_VERSION, params))
}

/// Drive one ec2Query action that must succeed, and answer its body.
pub(crate) fn ec2_ok(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let answer = ec2(router, runtime, action, params);
    assert_eq!(
        answer.status, 200,
        "setup: EC2 {action} must succeed, got {answer:?}"
    );
    answer.text
}

/// Drive one awsQuery action against the ELBv2 model, addressed by its own routing key so
/// this suite does not depend on which front door claims the shared signing name.
pub(crate) fn elbv2(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    send(
        router,
        runtime,
        "elasticloadbalancingv2",
        form(action, ELB_VERSION, params),
    )
}

/// The network every case runs against: one VPC with two subnets in different zones, and
/// a second VPC with a subnet and a security group of its own.
struct Network {
    vpc: String,
    /// `(subnet id, zone name)` for each subnet of [`Network::vpc`], in the order created.
    subnets: Vec<(String, String)>,
    /// A security group of [`Network::vpc`].
    security_group: String,
    other_vpc: String,
    other_subnet: String,
    /// A security group of [`Network::other_vpc`].
    other_security_group: String,
}

/// Create the network through EC2 itself, so every value this suite asserts against is one
/// EC2 reported rather than one the test made up.
fn seed_network(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) -> Network {
    let vpc = element(
        &ec2_ok(router, runtime, "CreateVpc", &[("CidrBlock", "10.80.0.0/16")]),
        "vpcId",
    );
    let mut subnets = Vec::new();
    for (cidr, zone) in [("10.80.1.0/24", "us-west-2a"), ("10.80.2.0/24", "us-west-2b")] {
        let created = ec2_ok(
            router,
            runtime,
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
    let security_group = element(
        &ec2_ok(
            router,
            runtime,
            "CreateSecurityGroup",
            &[
                ("GroupName", "elbv2-network-own"),
                ("GroupDescription", "the load balancers own VPC"),
                ("VpcId", vpc.as_str()),
            ],
        ),
        "groupId",
    );

    let other_vpc = element(
        &ec2_ok(router, runtime, "CreateVpc", &[("CidrBlock", "10.90.0.0/16")]),
        "vpcId",
    );
    let other_subnet = element(
        &ec2_ok(
            router,
            runtime,
            "CreateSubnet",
            &[
                ("VpcId", other_vpc.as_str()),
                ("CidrBlock", "10.90.1.0/24"),
                ("AvailabilityZone", "us-west-2a"),
            ],
        ),
        "subnetId",
    );
    let other_security_group = element(
        &ec2_ok(
            router,
            runtime,
            "CreateSecurityGroup",
            &[
                ("GroupName", "elbv2-network-foreign"),
                ("GroupDescription", "another VPC"),
                ("VpcId", other_vpc.as_str()),
            ],
        ),
        "groupId",
    );

    Network {
        vpc,
        subnets,
        security_group,
        other_vpc,
        other_subnet,
        other_security_group,
    }
}

/// Create an `ip` or `instance` target group in `vpc` and answer its ARN.
pub(crate) fn target_group(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    target_type: &str,
    vpc: &str,
) -> String {
    let created = elbv2(
        router,
        runtime,
        "CreateTargetGroup",
        &[
            ("Name", name),
            ("Protocol", "HTTP"),
            ("Port", "80"),
            ("TargetType", target_type),
            ("VpcId", vpc),
        ],
    );
    assert_eq!(
        created.status, 200,
        "setup: CreateTargetGroup {name} must succeed, got {created:?}"
    );
    element(&created.text, "TargetGroupArn")
}

/// Register one target, optionally naming its Availability Zone.
pub(crate) fn register(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
    id: &str,
    zone: Option<&str>,
) -> Answer {
    let mut params = vec![("TargetGroupArn", group), ("Targets.member.1.Id", id)];
    if let Some(zone) = zone {
        params.push(("Targets.member.1.AvailabilityZone", zone));
    }
    elbv2(router, runtime, "RegisterTargets", &params)
}

/// The `(target id, Availability Zone)` pairs `DescribeTargetHealth` reports for a group.
pub(crate) fn target_zones(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    group: &str,
) -> Vec<(String, String)> {
    let health = elbv2(
        router,
        runtime,
        "DescribeTargetHealth",
        &[("TargetGroupArn", group)],
    );
    assert_eq!(
        health.status, 200,
        "DescribeTargetHealth must succeed, got {health:?}"
    );
    let ids = elements(&health.text, "Id");
    let zones = elements(&health.text, "AvailabilityZone");
    assert_eq!(
        ids.len(),
        zones.len(),
        "every reported target must carry a zone: {}",
        health.text
    );
    ids.into_iter().zip(zones).collect()
}

/// One row of the `ip` target table: the case name, the address, the zone the request
/// names, and either the zone the registration must report or the refusal it earns.
struct IpRow {
    case: &'static str,
    address: &'static str,
    requested_zone: Option<&'static str>,
    /// `Ok(zone)` — accepted, and the registration reports this zone (`""` meaning "the
    /// zone of the subnet named by index"); `Err(message)` — refused with exactly this
    /// message, because WHICH refusal the address earns is the behaviour under test.
    expected: Result<&'static str, &'static str>,
}

#[test]
fn elbv2_resolves_the_account_network_against_ec2() {
    std::thread::Builder::new()
        .name("elbv2-network-resolution".to_string())
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
            let network = seed_network(&router, &runtime);

            // -----------------------------------------------------------------
            // The load balancer's VPC and zones come from its subnets.
            // -----------------------------------------------------------------
            let created = elbv2(
                &router,
                &runtime,
                "CreateLoadBalancer",
                &[
                    ("Name", "network-alb"),
                    ("Scheme", "internal"),
                    ("Subnets.member.1", network.subnets[0].0.as_str()),
                    ("Subnets.member.2", network.subnets[1].0.as_str()),
                    ("SecurityGroups.member.1", network.security_group.as_str()),
                ],
            );
            assert_eq!(
                created.status, 200,
                "case \"a load balancer in the VPC's own subnets\": expected 200, got \
                 {created:?}"
            );
            let described = elbv2(
                &router,
                &runtime,
                "DescribeLoadBalancers",
                &[("Names.member.1", "network-alb")],
            );
            assert_eq!(
                described.status, 200,
                "case \"a load balancer in the VPC's own subnets\": the read must \
                 succeed, got {described:?}"
            );
            assert_eq!(
                opt_element(&described.text, "VpcId").as_deref(),
                Some(network.vpc.as_str()),
                "case \"a load balancer in the VPC's own subnets\": expected the VPC of \
                 its subnets, got {}",
                described.text
            );
            let reported_zones = elements(&described.text, "ZoneName");
            let subnet_zones: Vec<String> = network
                .subnets
                .iter()
                .map(|(_, zone)| zone.clone())
                .collect();
            assert_eq!(
                reported_zones, subnet_zones,
                "case \"a load balancer in the VPC's own subnets\": each zone must be \
                 the one EC2 reports for that subnet, got {}",
                described.text
            );

            // -----------------------------------------------------------------
            // A security group of another VPC cannot apply to those interfaces.
            // -----------------------------------------------------------------
            // (case, the security groups the request names, whether it is accepted)
            let security_group_rows: Vec<(&str, Vec<&str>, bool)> = vec![
                ("no security group at all", vec![], true),
                (
                    "a group of the subnets' own VPC",
                    vec![network.security_group.as_str()],
                    true,
                ),
                (
                    "a group of another VPC",
                    vec![network.other_security_group.as_str()],
                    false,
                ),
                (
                    "a group of each VPC",
                    vec![
                        network.security_group.as_str(),
                        network.other_security_group.as_str(),
                    ],
                    false,
                ),
            ];
            for (index, (row, groups, accepted)) in security_group_rows.iter().enumerate() {
                // Both doors a client reaches this model through: the emulator's own ELBv2
                // route key, and the shared `elasticloadbalancing` signing name every AWS
                // SDK ELBv2 client signs for. The request is the same, so the answer is —
                // a placement an SDK client makes is judged on its network, never on which
                // front door claims the name it signed for.
                for (door_index, door) in ELBV2_DOORS.iter().enumerate() {
                    let case = format!("{row}, addressed as {door}");
                    let name = format!("sg-alb-{index}-{door_index}");
                    let mut params = vec![
                        ("Name", name.as_str()),
                        ("Scheme", "internal"),
                        ("Type", "application"),
                        ("Subnets.member.1", network.subnets[0].0.as_str()),
                        ("Subnets.member.2", network.subnets[1].0.as_str()),
                    ];
                    let keys = ["SecurityGroups.member.1", "SecurityGroups.member.2"];
                    for (key, group) in keys.iter().zip(groups) {
                        params.push((key, group));
                    }
                    let answer = send(
                        &router,
                        &runtime,
                        door,
                        form("CreateLoadBalancer", ELB_VERSION, &params),
                    );
                    if *accepted {
                        assert_eq!(
                            answer.status, 200,
                            "case {case:?}: expected the create to be accepted, got {answer:?}"
                        );
                    } else {
                        assert_eq!(
                            (answer.status, answer.code()),
                            (400, "InvalidConfigurationRequest".to_string()),
                            "case {case:?}: expected 400 InvalidConfigurationRequest, got \
                             {answer:?}"
                        );
                        // Nothing was created: the name is still free.
                        let read = send(
                            &router,
                            &runtime,
                            door,
                            form(
                                "DescribeLoadBalancers",
                                ELB_VERSION,
                                &[("Names.member.1", name.as_str())],
                            ),
                        );
                        assert_eq!(
                            (read.status, read.code()),
                            (400, "LoadBalancerNotFound".to_string()),
                            "case {case:?}: a refused create must store no load balancer, \
                             got {read:?}"
                        );
                    }
                }
            }

            // -----------------------------------------------------------------
            // An `ip` target is an address of the group's VPC, or one of a
            // private range with a zone named for it.
            // -----------------------------------------------------------------
            let ip_group = target_group(&router, &runtime, "ip-network-tg", "ip", &network.vpc);
            let rows = [
                IpRow {
                    case: "an address in the first subnet",
                    address: "10.80.1.5",
                    requested_zone: None,
                    expected: Ok("us-west-2a"),
                },
                IpRow {
                    case: "an address in the second subnet",
                    address: "10.80.2.7",
                    requested_zone: None,
                    expected: Ok("us-west-2b"),
                },
                IpRow {
                    // The containing subnet decides the zone, so a request naming
                    // another one does not move the target.
                    case: "an address in the second subnet with the wrong zone named",
                    address: "10.80.2.8",
                    requested_zone: Some("us-west-2a"),
                    expected: Ok("us-west-2b"),
                },
                IpRow {
                    case: "an address in the VPC CIDR that no subnet holds",
                    address: "10.80.9.9",
                    requested_zone: None,
                    expected: Err(
                        "The IP address '10.80.9.9' is within the VPC, but is not within a \
                         VPC subnet so it can't be registered",
                    ),
                },
                IpRow {
                    // Naming a zone is no remedy for this class either: the address is in
                    // no subnet of the VPC whatever the request says.
                    case: "an address in the VPC CIDR that no subnet holds, with a zone",
                    address: "10.80.9.10",
                    requested_zone: Some("us-west-2a"),
                    expected: Err(
                        "The IP address '10.80.9.10' is within the VPC, but is not within \
                         a VPC subnet so it can't be registered",
                    ),
                },
                IpRow {
                    case: "a private address outside the VPC with the zone to serve it",
                    address: "192.168.5.5",
                    requested_zone: Some("us-west-2a"),
                    expected: Ok("us-west-2a"),
                },
                IpRow {
                    case: "a private address outside the VPC with no zone",
                    address: "192.168.5.6",
                    requested_zone: None,
                    expected: Err(
                        "You must specify a valid Availability Zone or select 'all' for \
                         all enabled Availability zones, since the IP address \
                         '192.168.5.6' is outside the VPC",
                    ),
                },
                IpRow {
                    case: "an address of the RFC 6598 range with a zone",
                    address: "100.64.3.3",
                    requested_zone: Some("us-west-2b"),
                    expected: Ok("us-west-2b"),
                },
                IpRow {
                    case: "a publicly routable address with a zone",
                    address: "8.8.8.8",
                    requested_zone: Some("us-west-2a"),
                    expected: Err(
                        "The IP address '8.8.8.8' is not allowed, as it must be an address \
                         from a VPC subnet, RFC1918, or RFC6598",
                    ),
                },
                IpRow {
                    case: "a publicly routable address of another range",
                    address: "203.0.113.4",
                    requested_zone: Some("us-west-2b"),
                    expected: Err(
                        "The IP address '203.0.113.4' is not allowed, as it must be an \
                         address from a VPC subnet, RFC1918, or RFC6598",
                    ),
                },
                IpRow {
                    // Whether the address is one an `ip` target may carry at all does not
                    // depend on the zone, so it is judged first: such an address is
                    // refused on its own terms rather than asked for a zone that would not
                    // make it registrable.
                    case: "a publicly routable address with no zone",
                    address: "1.1.1.1",
                    requested_zone: None,
                    expected: Err(
                        "The IP address '1.1.1.1' is not allowed, as it must be an address \
                         from a VPC subnet, RFC1918, or RFC6598",
                    ),
                },
                IpRow {
                    // All zones is no remedy either: the class is chosen by the address.
                    case: "a publicly routable address with all zones",
                    address: "192.0.2.10",
                    requested_zone: Some("all"),
                    expected: Err(
                        "The IP address '192.0.2.10' is not allowed, as it must be an \
                         address from a VPC subnet, RFC1918, or RFC6598",
                    ),
                },
                IpRow {
                    // And all zones IS the remedy for the address class that has one.
                    case: "a private address outside the VPC with all zones",
                    address: "192.168.5.7",
                    requested_zone: Some("all"),
                    expected: Ok("all"),
                },
            ];
            let mut accepted: Vec<(String, String)> = Vec::new();
            for row in &rows {
                let answer = register(&router, &runtime, &ip_group, row.address, row.requested_zone);
                match row.expected {
                    Ok(zone) => {
                        assert_eq!(
                            answer.status, 200,
                            "case {:?}: expected the registration to be accepted, got \
                             {answer:?}",
                            row.case
                        );
                        accepted.push((row.address.to_string(), zone.to_string()));
                    }
                    Err(message) => {
                        assert_eq!(
                            (answer.status, answer.code()),
                            (400, "ValidationError".to_string()),
                            "case {:?}: expected 400 ValidationError, got {answer:?}",
                            row.case
                        );
                        assert_eq!(
                            answer.message(),
                            message,
                            "case {:?}: expected the refusal {message:?}, got {:?}",
                            row.case,
                            answer.message()
                        );
                    }
                }
                // Only the accepted registrations are there, each with its resolved zone.
                let mut held = target_zones(&router, &runtime, &ip_group);
                held.sort();
                let mut want = accepted.clone();
                want.sort();
                assert_eq!(
                    held, want,
                    "case {:?}: the group must hold exactly the accepted registrations",
                    row.case
                );
            }

            // -----------------------------------------------------------------
            // An `instance` target is an instance of the group's own VPC.
            // -----------------------------------------------------------------
            let launched = ec2_ok(
                &router,
                &runtime,
                "RunInstances",
                &[
                    ("ImageId", AMI),
                    ("MinCount", "1"),
                    ("MaxCount", "1"),
                    ("SubnetId", network.subnets[0].0.as_str()),
                ],
            );
            let instance = element(&launched, "instanceId");
            let elsewhere = element(
                &ec2_ok(
                    &router,
                    &runtime,
                    "RunInstances",
                    &[
                        ("ImageId", AMI),
                        ("MinCount", "1"),
                        ("MaxCount", "1"),
                        ("SubnetId", network.other_subnet.as_str()),
                    ],
                ),
                "instanceId",
            );
            let instance_group =
                target_group(&router, &runtime, "inst-network-tg", "instance", &network.vpc);
            // (case, the target id, the awsQuery code the registration is refused under —
            // empty for an id that is accepted)
            //
            // An instance EC2 holds in another VPC EXISTS and cannot be registered: the
            // modelled `InvalidTarget`. An id that is not WRITTEN as an instance id, and one
            // EC2 holds no instance for, each name no target of any account, so they are the
            // request to fix: `ValidationError`.
            let instance_rows: Vec<(&str, &str, &str)> = vec![
                ("an instance of the group's own VPC", instance.as_str(), ""),
                (
                    "an instance of another VPC",
                    elsewhere.as_str(),
                    "InvalidTarget",
                ),
                (
                    "an id no instance of the account carries",
                    "i-0123456789abcdef0",
                    "ValidationError",
                ),
                (
                    "an id that is no instance id at all",
                    "not-an-instance",
                    "ValidationError",
                ),
            ];
            for (case, id, code) in instance_rows {
                let answer = register(&router, &runtime, &instance_group, id, None);
                if code.is_empty() {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: expected the registration to be accepted, got \
                         {answer:?}"
                    );
                } else {
                    assert_eq!(
                        (answer.status, answer.code()),
                        (400, code.to_string()),
                        "case {case:?}: expected 400 {code}, got {answer:?}"
                    );
                }
            }
            let held: Vec<String> = elements(
                &elbv2(
                    &router,
                    &runtime,
                    "DescribeTargetHealth",
                    &[("TargetGroupArn", instance_group.as_str())],
                )
                .text,
                "Id",
            );
            assert_eq!(
                held,
                vec![instance.clone()],
                "only the instance of the group's own VPC may be registered"
            );

            // The SAME instance is refused by a target group of the other VPC, so the
            // rule is about the pairing rather than about the instance.
            let foreign_group = target_group(
                &router,
                &runtime,
                "inst-foreign-tg",
                "instance",
                &network.other_vpc,
            );
            let answer = register(&router, &runtime, &foreign_group, &instance, None);
            assert_eq!(
                (answer.status, answer.code()),
                (400, "InvalidTarget".to_string()),
                "case \"an instance of another VPC than the group's\": expected 400 \
                 InvalidTarget, got {answer:?}"
            );
        })
        .expect("the sized thread spawns")
        .join()
        .expect("the suite thread does not panic");
}
