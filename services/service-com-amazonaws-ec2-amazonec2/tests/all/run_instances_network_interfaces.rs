// SPDX-License-Identifier: CC0-1.0

//! `RunInstances` with a NETWORK INTERFACE SPECIFICATION
//! (`RunInstancesRequest$NetworkInterfaces`), driven over the wire the service speaks
//! (`ec2Query`: an `application/x-www-form-urlencoded` POST answered with XML — README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! ONE RULE is under test: a launch that names the interfaces of its instance launches each
//! instance with exactly those interfaces — each CREATED in the subnet its entry names, with
//! the security groups, private IPv4 addresses, description, type and options the entry names
//! and attached at its own device index, or, for an entry that names an interface by id, that
//! EXISTING interface attached where it is — and the interface at device index 0 is the
//! instance's primary one, so the instance's subnet, groups and primary private address are
//! its. They are one record, so `DescribeInstances` and `DescribeNetworkInterfaces` report the
//! same interface.
//!
//! The rows span what that rule generalises over: how many interfaces a launch names, whether
//! an entry names its subnet or lets the launch choose one, whether it names addresses or a
//! count of them, which groups it applies, its description and type, whether it asks for a
//! public IPv4 address, whether the interface is created or attached by id, which order the
//! entries arrive in, and how many instances one launch mints. The recorded case — one eth0
//! naming a subnet, a group, `AssociatePublicIpAddress` and `DeleteOnTermination` — is one row
//! among them.
//!
//! The refusal table beside them covers the other half of the same rule: what a launch asks
//! for that an interface cannot be given is refused, naming the parameter at fault, rather
//! than served with an instance whose interfaces are not what was asked for.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, indexed};
use super::wire::{Param, child, child_text, document, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The image every launch names: `RunInstances` only has to accept the id, and which image an
/// instance was launched from has no bearing on its interfaces.
const AMI: &str = "ami-0123456789abcdef0";

/// The CIDR block of the VPC this suite creates, outside the `172.31.0.0/16` a default VPC
/// has, so a reported address says which network it came from.
const VPC_CIDR: &str = "10.70.0.0/16";

/// The two subnets this suite creates in ONE zone of that VPC, and the one it creates in
/// another zone (for the row where an interface would sit outside the instance's zone).
const SUBNET_A_CIDR: &str = "10.70.1.0/24";
const SUBNET_B_CIDR: &str = "10.70.2.0/24";
const SUBNET_OTHER_ZONE_CIDR: &str = "10.70.3.0/24";

/// A well-formed id of a resource no account holds, for the rows that must be refused with a
/// not-found rather than served.
const UNKNOWN_SUBNET: &str = "subnet-00000000000000000";
const UNKNOWN_GROUP: &str = "sg-00000000000000000";
const UNKNOWN_INTERFACE: &str = "eni-00000000000000000";

/// One call that must succeed, as its body.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The network this suite launches into: one VPC, two subnets in one zone, a third in
/// another, two groups of this VPC and one of a second VPC.
struct Network {
    router: MultiServiceRouter,
    subnet_a: String,
    subnet_b: String,
    subnet_other_zone: String,
    group_a: String,
    group_b: String,
    foreign_group: String,
    zone_a: String,
    zone_b: String,
}

impl Network {
    async fn create() -> Self {
        let (router, _engine) = super::ec2_query::emulator();
        let zones = ec2::state::availability_zones_in_region(REGION);
        let zone_a = zones
            .first()
            .map(|zone| zone.zone_name.clone())
            .expect("a Region reports Availability Zones");
        let zone_b = zones
            .get(1)
            .map(|zone| zone.zone_name.clone())
            .expect("a Region reports more than one Availability Zone");
        let vpc_id = {
            let body = ok("the VPC", &router, "CreateVpc", &[param("CidrBlock", VPC_CIDR)]).await;
            element(&body, "vpcId").unwrap_or_default()
        };
        let subnet = async |cidr_block: &str, zone: &str| -> String {
            let body = ok(
                "the subnet",
                &router,
                "CreateSubnet",
                &[
                    param("VpcId", &vpc_id),
                    param("CidrBlock", cidr_block),
                    param("AvailabilityZone", zone),
                ],
            )
            .await;
            element(&body, "subnetId").unwrap_or_default()
        };
        let subnet_a = subnet(SUBNET_A_CIDR, &zone_a).await;
        let subnet_b = subnet(SUBNET_B_CIDR, &zone_a).await;
        let subnet_other_zone = subnet(SUBNET_OTHER_ZONE_CIDR, &zone_b).await;
        let group = async |vpc_id: &str, name: &str| -> String {
            let body = ok(
                "the group",
                &router,
                "CreateSecurityGroup",
                &[
                    param("VpcId", vpc_id),
                    param("GroupName", name),
                    param("GroupDescription", "a group this suite launches with"),
                ],
            )
            .await;
            element(&body, "groupId").unwrap_or_default()
        };
        let group_a = group(&vpc_id, "interfaces-a").await;
        let group_b = group(&vpc_id, "interfaces-b").await;
        let foreign_vpc = {
            let body = ok(
                "the second VPC",
                &router,
                "CreateVpc",
                &[param("CidrBlock", "10.80.0.0/16")],
            )
            .await;
            element(&body, "vpcId").unwrap_or_default()
        };
        let foreign_group = group(&foreign_vpc, "interfaces-elsewhere").await;
        Self {
            router,
            subnet_a,
            subnet_b,
            subnet_other_zone,
            group_a,
            group_b,
            foreign_group,
            zone_a,
            zone_b,
        }
    }

    /// One `RunInstances` call that must succeed, as its body.
    async fn launch(&self, case: &str, count: u32, interfaces: &[Param]) -> String {
        let mut params = vec![
            param("ImageId", AMI),
            param("MinCount", &count.to_string()),
            param("MaxCount", &count.to_string()),
        ];
        params.extend_from_slice(interfaces);
        ok(case, &self.router, "RunInstances", &params).await
    }

    /// One `RunInstances` call that must be refused, as its status and error code.
    async fn refused_launch(&self, count: u32, interfaces: &[Param]) -> (u16, String) {
        let mut params = vec![
            param("ImageId", AMI),
            param("MinCount", &count.to_string()),
            param("MaxCount", &count.to_string()),
        ];
        params.extend_from_slice(interfaces);
        let (status, body) = call(&self.router, REGION, "RunInstances", &params).await;
        (status, error_code(&body))
    }
}

/// One instance as this suite reads it, from a launch's own answer or from a describe.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedInstance {
    instance_id: String,
    subnet_id: Option<String>,
    vpc_id: Option<String>,
    private_ip: Option<String>,
    public_ip: Option<String>,
    public_dns_name: Option<String>,
    zone: Option<String>,
    group_ids: Vec<String>,
    interfaces: Vec<ReportedInterface>,
}

/// One entry of an instance's `networkInterfaceSet`, or the same interface as
/// `DescribeNetworkInterfaces` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedInterface {
    network_interface_id: String,
    subnet_id: Option<String>,
    vpc_id: Option<String>,
    description: Option<String>,
    interface_type: Option<String>,
    status: Option<String>,
    private_ip: Option<String>,
    addresses: Vec<String>,
    group_ids: Vec<String>,
    device_index: Option<String>,
    network_card_index: Option<String>,
    delete_on_termination: Option<String>,
    attachment_status: Option<String>,
    attached_instance: Option<String>,
    association_public_ip: Option<String>,
    association_owner: Option<String>,
}

/// Read one `<networkInterface>`-shaped element.
fn reported_interface(node: roxmltree::Node<'_, '_>) -> ReportedInterface {
    let attachment = child(node, "attachment");
    let association = child(node, "association");
    ReportedInterface {
        network_interface_id: child_text(node, "networkInterfaceId").unwrap_or_default(),
        subnet_id: child_text(node, "subnetId"),
        vpc_id: child_text(node, "vpcId"),
        description: child_text(node, "description"),
        interface_type: child_text(node, "interfaceType"),
        status: child_text(node, "status"),
        private_ip: child_text(node, "privateIpAddress"),
        addresses: set_items(node, "privateIpAddressesSet")
            .into_iter()
            .filter_map(|address| child_text(address, "privateIpAddress"))
            .collect(),
        group_ids: set_items(node, "groupSet")
            .into_iter()
            .filter_map(|group| child_text(group, "groupId"))
            .collect(),
        device_index: attachment.and_then(|held| child_text(held, "deviceIndex")),
        network_card_index: attachment.and_then(|held| child_text(held, "networkCardIndex")),
        delete_on_termination: attachment.and_then(|held| child_text(held, "deleteOnTermination")),
        attachment_status: attachment.and_then(|held| child_text(held, "status")),
        attached_instance: attachment.and_then(|held| child_text(held, "instanceId")),
        association_public_ip: association.and_then(|held| child_text(held, "publicIp")),
        association_owner: association.and_then(|held| child_text(held, "ipOwnerId")),
    }
}

/// Every instance a body reports — a `RunInstances` answer carries one reservation, a
/// `DescribeInstances` answer carries a set of them.
fn reported_instances(case: &str, body: &str) -> Vec<ReportedInstance> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && node.tag_name().name() == "item"
                && child(*node, "instanceId").is_some()
                && child(*node, "instanceState").is_some()
        })
        .map(|node| {
            let mut interfaces: Vec<ReportedInterface> = set_items(node, "networkInterfaceSet")
                .into_iter()
                .map(reported_interface)
                .collect();
            interfaces.sort_by(|left, right| left.device_index.cmp(&right.device_index));
            ReportedInstance {
                instance_id: child_text(node, "instanceId").unwrap_or_default(),
                subnet_id: child_text(node, "subnetId"),
                vpc_id: child_text(node, "vpcId"),
                private_ip: child_text(node, "privateIpAddress"),
                // `Instance$PublicIpAddress` is `<ipAddress>` on the wire and
                // `Instance$PublicDnsName` is `<dnsName>`.
                public_ip: child_text(node, "ipAddress"),
                public_dns_name: child_text(node, "dnsName"),
                zone: child(node, "placement")
                    .and_then(|placement| child_text(placement, "availabilityZone")),
                group_ids: set_items(node, "groupSet")
                    .into_iter()
                    .filter_map(|group| child_text(group, "groupId"))
                    .collect(),
                interfaces,
            }
        })
        .collect()
}

/// The instances one launch minted, read off the launch's own answer.
fn launched(case: &str, body: &str) -> Vec<ReportedInstance> {
    let reported = reported_instances(case, body);
    assert!(
        !reported.is_empty(),
        "case {case:?}: the launch reported no instance: {body}"
    );
    reported
}

/// `DescribeInstances` narrowed to one instance, as that instance.
async fn described(case: &str, router: &MultiServiceRouter, instance_id: &str) -> ReportedInstance {
    let body = ok(
        case,
        router,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id]),
    )
    .await;
    let mut reported = reported_instances(case, &body);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly one instance for {instance_id}, got {reported:?} \
         from {body}"
    );
    reported.pop().expect("checked just above")
}

/// `DescribeNetworkInterfaces` narrowed to one interface, as that interface.
async fn described_interface(
    case: &str,
    router: &MultiServiceRouter,
    network_interface_id: &str,
) -> ReportedInterface {
    let body = ok(
        case,
        router,
        "DescribeNetworkInterfaces",
        &indexed("NetworkInterfaceId", &[network_interface_id]),
    )
    .await;
    let parsed = document(case, &body);
    let node = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "networkInterfaceId").is_some())
        .unwrap_or_else(|| {
            panic!("case {case:?}: no interface for {network_interface_id} in {body}")
        });
    reported_interface(node)
}

/// The form fields of one `NetworkInterfaces` entry: `NetworkInterface.N.<member>`, as
/// ec2Query flattens a list of structures (`RunInstancesRequest$NetworkInterfaces` carries the
/// `ec2QueryName` `NetworkInterface`).
fn interface(index: usize, members: &[(&str, &str)]) -> Vec<Param> {
    members
        .iter()
        .map(|(name, value)| param(&format!("NetworkInterface.{index}.{name}"), value))
        .collect()
}

/// What one row expects of one interface of the launched instance.
struct ExpectedInterface {
    device_index: &'static str,
    subnet_id: String,
    description: Option<&'static str>,
    interface_type: &'static str,
    group_ids: Vec<String>,
    address_count: usize,
    delete_on_termination: &'static str,
    public_address: bool,
}

/// One launch row: the interfaces it names, and what the instance must then report.
struct Row {
    case: &'static str,
    count: u32,
    fields: Vec<Param>,
    expected: Vec<ExpectedInterface>,
}

/// THE RULE: every interface a launch names is created as named and reported on the instance,
/// and `DescribeNetworkInterfaces` reports the same record.
#[tokio::test]
async fn the_interfaces_a_launch_names_are_created_and_reported() {
    let network = Network::create().await;
    let rows = vec![
        Row {
            // The recorded shape: one eth0 pinning the subnet, a group, the public-IP
            // association and delete-on-termination.
            case: "one eth0 naming a subnet, a group and a public address",
            count: 1,
            fields: interface(
                1,
                &[
                    ("DeviceIndex", "0"),
                    ("SubnetId", &network.subnet_a),
                    ("SecurityGroupId.1", &network.group_a),
                    ("AssociatePublicIpAddress", "true"),
                    ("DeleteOnTermination", "true"),
                ],
            ),
            expected: vec![ExpectedInterface {
                device_index: "0",
                subnet_id: network.subnet_a.clone(),
                description: Some(""),
                interface_type: "interface",
                group_ids: vec![network.group_a.clone()],
                address_count: 1,
                delete_on_termination: "true",
                public_address: true,
            }],
        },
        Row {
            case: "an eth0 that names no subnet, which lands in the zone's default subnet",
            count: 1,
            fields: interface(1, &[("DeviceIndex", "0")]),
            // The default subnet of the launch's zone, resolved below from the instance.
            expected: vec![ExpectedInterface {
                device_index: "0",
                subnet_id: String::new(),
                description: Some(""),
                interface_type: "interface",
                group_ids: Vec::new(),
                address_count: 1,
                delete_on_termination: "true",
                public_address: false,
            }],
        },
        Row {
            case: "an eth0 with a description, two groups and a type of its own",
            count: 1,
            fields: interface(
                1,
                &[
                    ("DeviceIndex", "0"),
                    ("SubnetId", &network.subnet_a),
                    ("SecurityGroupId.1", &network.group_a),
                    ("SecurityGroupId.2", &network.group_b),
                    ("Description", "the primary interface"),
                    ("InterfaceType", "efa"),
                ],
            ),
            expected: vec![ExpectedInterface {
                device_index: "0",
                subnet_id: network.subnet_a.clone(),
                description: Some("the primary interface"),
                interface_type: "efa",
                group_ids: vec![network.group_a.clone(), network.group_b.clone()],
                address_count: 1,
                delete_on_termination: "true",
                public_address: false,
            }],
        },
        Row {
            case: "an eth0 naming its primary address and two secondary ones",
            count: 1,
            fields: interface(
                1,
                &[
                    ("DeviceIndex", "0"),
                    ("SubnetId", &network.subnet_a),
                    ("PrivateIpAddress", "10.70.1.40"),
                    ("SecondaryPrivateIpAddressCount", "2"),
                ],
            ),
            expected: vec![ExpectedInterface {
                device_index: "0",
                subnet_id: network.subnet_a.clone(),
                description: Some(""),
                interface_type: "interface",
                group_ids: Vec::new(),
                address_count: 3,
                delete_on_termination: "true",
                public_address: false,
            }],
        },
        Row {
            case: "an eth0 and an eth1 that each ask for a secondary address",
            count: 1,
            fields: {
                let mut fields = interface(
                    1,
                    &[
                        ("DeviceIndex", "0"),
                        ("SubnetId", &network.subnet_a),
                        ("SecondaryPrivateIpAddressCount", "1"),
                    ],
                );
                fields.extend(interface(
                    2,
                    &[
                        ("DeviceIndex", "1"),
                        ("SubnetId", &network.subnet_b),
                        ("SecondaryPrivateIpAddressCount", "3"),
                    ],
                ));
                fields
            },
            expected: vec![
                ExpectedInterface {
                    device_index: "0",
                    subnet_id: network.subnet_a.clone(),
                    description: Some(""),
                    interface_type: "interface",
                    group_ids: Vec::new(),
                    address_count: 2,
                    delete_on_termination: "true",
                    public_address: false,
                },
                ExpectedInterface {
                    device_index: "1",
                    subnet_id: network.subnet_b.clone(),
                    description: Some(""),
                    interface_type: "interface",
                    group_ids: Vec::new(),
                    address_count: 4,
                    delete_on_termination: "true",
                    public_address: false,
                },
            ],
        },
        Row {
            case: "two interfaces, the second in another subnet of the same zone",
            count: 1,
            fields: {
                let mut fields = interface(
                    1,
                    &[
                        ("DeviceIndex", "0"),
                        ("SubnetId", &network.subnet_a),
                        ("SecurityGroupId.1", &network.group_a),
                        ("DeleteOnTermination", "false"),
                    ],
                );
                fields.extend(interface(
                    2,
                    &[
                        ("DeviceIndex", "1"),
                        ("SubnetId", &network.subnet_b),
                        ("SecurityGroupId.1", &network.group_b),
                        ("Description", "the second interface"),
                        ("NetworkCardIndex", "0"),
                    ],
                ));
                fields
            },
            expected: vec![
                ExpectedInterface {
                    device_index: "0",
                    subnet_id: network.subnet_a.clone(),
                    description: Some(""),
                    interface_type: "interface",
                    group_ids: vec![network.group_a.clone()],
                    address_count: 1,
                    delete_on_termination: "false",
                    public_address: false,
                },
                ExpectedInterface {
                    device_index: "1",
                    subnet_id: network.subnet_b.clone(),
                    description: Some("the second interface"),
                    interface_type: "interface",
                    group_ids: vec![network.group_b.clone()],
                    address_count: 1,
                    delete_on_termination: "true",
                    public_address: false,
                },
            ],
        },
        Row {
            case: "three interfaces listed in reverse device-index order",
            count: 1,
            fields: {
                let mut fields = interface(
                    1,
                    &[("DeviceIndex", "2"), ("SubnetId", &network.subnet_a)],
                );
                fields.extend(interface(
                    2,
                    &[("DeviceIndex", "1"), ("SubnetId", &network.subnet_b)],
                ));
                fields.extend(interface(
                    3,
                    &[("DeviceIndex", "0"), ("SubnetId", &network.subnet_a)],
                ));
                fields
            },
            expected: vec![
                ExpectedInterface {
                    device_index: "0",
                    subnet_id: network.subnet_a.clone(),
                    description: Some(""),
                    interface_type: "interface",
                    group_ids: Vec::new(),
                    address_count: 1,
                    delete_on_termination: "true",
                    public_address: false,
                },
                ExpectedInterface {
                    device_index: "1",
                    subnet_id: network.subnet_b.clone(),
                    description: Some(""),
                    interface_type: "interface",
                    group_ids: Vec::new(),
                    address_count: 1,
                    delete_on_termination: "true",
                    public_address: false,
                },
                ExpectedInterface {
                    device_index: "2",
                    subnet_id: network.subnet_a.clone(),
                    description: Some(""),
                    interface_type: "interface",
                    group_ids: Vec::new(),
                    address_count: 1,
                    delete_on_termination: "true",
                    public_address: false,
                },
            ],
        },
        Row {
            case: "three instances from one launch, each with its own two interfaces",
            count: 3,
            fields: {
                let mut fields = interface(
                    1,
                    &[("DeviceIndex", "0"), ("SubnetId", &network.subnet_a)],
                );
                fields.extend(interface(
                    2,
                    &[("DeviceIndex", "1"), ("SubnetId", &network.subnet_b)],
                ));
                fields
            },
            expected: vec![
                ExpectedInterface {
                    device_index: "0",
                    subnet_id: network.subnet_a.clone(),
                    description: Some(""),
                    interface_type: "interface",
                    group_ids: Vec::new(),
                    address_count: 1,
                    delete_on_termination: "true",
                    public_address: false,
                },
                ExpectedInterface {
                    device_index: "1",
                    subnet_id: network.subnet_b.clone(),
                    description: Some(""),
                    interface_type: "interface",
                    group_ids: Vec::new(),
                    address_count: 1,
                    delete_on_termination: "true",
                    public_address: false,
                },
            ],
        },
    ];

    let mut every_address: Vec<String> = Vec::new();
    for row in &rows {
        let case = row.case;
        let body = network.launch(case, row.count, &row.fields).await;
        let instances = launched(case, &body);
        assert_eq!(
            instances.len(),
            row.count as usize,
            "case {case:?}: expected {} instance(s), got {instances:?}",
            row.count
        );
        for instance in &instances {
            assert_eq!(
                instance.interfaces.len(),
                row.expected.len(),
                "case {case:?}: expected {} interface(s) on {}, got {:?}",
                row.expected.len(),
                instance.instance_id,
                instance.interfaces
            );
            for (expected, reported) in row.expected.iter().zip(&instance.interfaces) {
                assert_eq!(
                    reported.device_index.as_deref(),
                    Some(expected.device_index),
                    "case {case:?}: expected an interface at device index {}, got {reported:?}",
                    expected.device_index
                );
                if !expected.subnet_id.is_empty() {
                    assert_eq!(
                        reported.subnet_id.as_deref(),
                        Some(expected.subnet_id.as_str()),
                        "case {case:?}: expected the interface in subnet {}, got {reported:?}",
                        expected.subnet_id
                    );
                }
                assert_eq!(
                    reported.description.as_deref(),
                    expected.description,
                    "case {case:?}: expected the description {:?}, got {reported:?}",
                    expected.description
                );
                assert_eq!(
                    reported.interface_type.as_deref(),
                    Some(expected.interface_type),
                    "case {case:?}: expected the type {:?}, got {reported:?}",
                    expected.interface_type
                );
                assert_eq!(
                    reported.delete_on_termination.as_deref(),
                    Some(expected.delete_on_termination),
                    "case {case:?}: expected deleteOnTermination {:?}, got {reported:?}",
                    expected.delete_on_termination
                );
                assert_eq!(
                    reported.addresses.len(),
                    expected.address_count,
                    "case {case:?}: expected {} private address(es), got {reported:?}",
                    expected.address_count
                );
                // The groups the entry named, or — for an entry that named none — the VPC's
                // own default group, which is what "Default: Amazon EC2 uses the default
                // security group" gives an interface.
                if expected.group_ids.is_empty() {
                    assert_eq!(
                        reported.group_ids.len(),
                        1,
                        "case {case:?}: expected the VPC's default group on an interface that \
                         named none, got {reported:?}"
                    );
                } else {
                    assert_eq!(
                        reported.group_ids, expected.group_ids,
                        "case {case:?}: expected the groups {:?}, got {reported:?}",
                        expected.group_ids
                    );
                }
                assert_eq!(
                    reported.association_public_ip.is_some(),
                    expected.public_address,
                    "case {case:?}: expected a public address: {}, got {reported:?}",
                    expected.public_address
                );
                every_address.extend(reported.addresses.clone());

                // ONE RECORD: the interface the instance reports is the interface
                // `DescribeNetworkInterfaces` reports, attached to this instance.
                let read_back = described_interface(
                    case,
                    &network.router,
                    &reported.network_interface_id,
                )
                .await;
                assert_eq!(
                    read_back.subnet_id, reported.subnet_id,
                    "case {case:?}: the describe of {} reports another subnet: {read_back:?}",
                    reported.network_interface_id
                );
                assert_eq!(
                    read_back.addresses, reported.addresses,
                    "case {case:?}: the describe of {} reports other addresses: {read_back:?}",
                    reported.network_interface_id
                );
                assert_eq!(
                    read_back.group_ids, reported.group_ids,
                    "case {case:?}: the describe of {} reports other groups: {read_back:?}",
                    reported.network_interface_id
                );
                assert_eq!(
                    read_back.attached_instance.as_deref(),
                    Some(instance.instance_id.as_str()),
                    "case {case:?}: expected {} attached to {}, got {read_back:?}",
                    reported.network_interface_id,
                    instance.instance_id
                );
                assert_eq!(
                    read_back.status.as_deref(),
                    Some("in-use"),
                    "case {case:?}: an attached interface is in-use, got {read_back:?}"
                );
            }

            // The PRIMARY interface is the instance's own network: its subnet, its groups and
            // its primary private address are the instance's.
            let primary = instance
                .interfaces
                .first()
                .expect("every row names an interface at device index 0");
            assert_eq!(
                instance.subnet_id, primary.subnet_id,
                "case {case:?}: expected the instance in its primary interface's subnet, got \
                 {instance:?}"
            );
            assert_eq!(
                instance.private_ip, primary.private_ip,
                "case {case:?}: expected the instance's address to be its primary interface's, \
                 got {instance:?}"
            );
            assert_eq!(
                instance.group_ids, primary.group_ids,
                "case {case:?}: expected the instance's groups to be its primary interface's, \
                 got {instance:?}"
            );
            assert_eq!(
                instance.vpc_id, primary.vpc_id,
                "case {case:?}: expected the instance in its primary interface's VPC, got \
                 {instance:?}"
            );

            // The launch reported the STORE: the describe answers the same interfaces.
            let read_back = described(case, &network.router, &instance.instance_id).await;
            assert_eq!(
                read_back.interfaces, instance.interfaces,
                "case {case:?}: the describe of {} reports other interfaces",
                instance.instance_id
            );
        }
    }

    // No address was handed out twice across every interface of every launch above: they all
    // come out of the one pool.
    let mut unique = every_address.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        every_address.len(),
        "expected every interface address to be unique, got {every_address:?}"
    );
}

/// THE RULE, the attach half: an entry that names an interface by id attaches THAT interface
/// rather than creating one — it keeps its own subnet, addresses and groups, and nothing new
/// is stored.
#[tokio::test]
async fn an_existing_interface_the_launch_names_is_attached_where_it_is() {
    let network = Network::create().await;
    let case = "an existing interface attached at launch";
    let created = ok(
        case,
        &network.router,
        "CreateNetworkInterface",
        &[
            param("SubnetId", &network.subnet_a),
            param("Description", "created before the launch"),
            param("PrivateIpAddress", "10.70.1.77"),
            param("SecurityGroupId.1", &network.group_b),
        ],
    )
    .await;
    let network_interface_id = element(&created, "networkInterfaceId").unwrap_or_default();
    let before = described_interface(case, &network.router, &network_interface_id).await;
    assert_eq!(
        before.status.as_deref(),
        Some("available"),
        "case {case:?}: a created interface holds nothing yet, got {before:?}"
    );

    let body = network
        .launch(
            case,
            1,
            &interface(
                1,
                &[
                    ("DeviceIndex", "0"),
                    ("NetworkInterfaceId", &network_interface_id),
                ],
            ),
        )
        .await;
    let instances = launched(case, &body);
    let instance = &instances[0];
    assert_eq!(
        instance.interfaces.len(),
        1,
        "case {case:?}: expected the one attached interface, got {instance:?}"
    );
    let attached = &instance.interfaces[0];
    assert_eq!(
        attached.network_interface_id, network_interface_id,
        "case {case:?}: expected the launch to attach {network_interface_id}, got {attached:?}"
    );
    assert_eq!(
        attached.addresses, before.addresses,
        "case {case:?}: an attached interface keeps its addresses, got {attached:?}"
    );
    assert_eq!(
        attached.group_ids, before.group_ids,
        "case {case:?}: an attached interface keeps its groups, got {attached:?}"
    );
    assert_eq!(
        attached.description.as_deref(),
        Some("created before the launch"),
        "case {case:?}: an attached interface keeps its description, got {attached:?}"
    );
    // "If you attach a network interface to an existing instance, the DeleteOnTermination
    // attribute is set to false by default": only an interface the launch CREATES goes with
    // its instance.
    assert_eq!(
        attached.delete_on_termination.as_deref(),
        Some("false"),
        "case {case:?}: expected deleteOnTermination false, got {attached:?}"
    );
    assert_eq!(
        instance.private_ip.as_deref(),
        Some("10.70.1.77"),
        "case {case:?}: the instance's address is its primary interface's, got {instance:?}"
    );

    // Nothing new was stored: the account still holds exactly the interfaces it held, and the
    // one it held is now in use by this instance.
    let read_back = described_interface(case, &network.router, &network_interface_id).await;
    assert_eq!(
        read_back.attached_instance.as_deref(),
        Some(instance.instance_id.as_str()),
        "case {case:?}: expected {network_interface_id} attached to {}, got {read_back:?}",
        instance.instance_id
    );
    let interfaces_of_subnet = ok(
        case,
        &network.router,
        "DescribeNetworkInterfaces",
        &super::ec2_query::filters(&[("subnet-id", &[network.subnet_a.as_str()])]),
    )
    .await;
    assert_eq!(
        elements(&interfaces_of_subnet, "networkInterfaceId"),
        vec![network_interface_id.clone()],
        "case {case:?}: expected the subnet to hold only the interface created before the \
         launch: {interfaces_of_subnet}"
    );
}

/// THE RULE, the public-address half: an eth0 that asks for a public IPv4 address gets one
/// out of Amazon's pool — reported by the instance, by its interface's association and as
/// the interface's owner — and one that does not ask gets none.
#[tokio::test]
async fn a_launch_asks_its_primary_interface_for_a_public_address() {
    let network = Network::create().await;
    for (case, asked, expect_address) in [
        ("an eth0 that asks for a public address", Some("true"), true),
        (
            "an eth0 that asks for none explicitly",
            Some("false"),
            false,
        ),
        ("an eth0 that does not ask at all", None, false),
    ] {
        let mut members = vec![("DeviceIndex", "0"), ("SubnetId", network.subnet_a.as_str())];
        if let Some(asked) = asked {
            members.push(("AssociatePublicIpAddress", asked));
        }
        let body = network.launch(case, 1, &interface(1, &members)).await;
        let instances = launched(case, &body);
        let instance = &instances[0];
        let reported = &instance.interfaces[0];
        assert_eq!(
            instance.public_ip.is_some(),
            expect_address,
            "case {case:?}: expected a public address: {expect_address}, got {instance:?}"
        );
        assert_eq!(
            reported.association_public_ip, instance.public_ip,
            "case {case:?}: expected the instance's public address to be its interface's, got \
             {instance:?}"
        );
        if expect_address {
            // An address AWS assigned itself is owned by `amazon`, where an Elastic IP is
            // owned by the account that allocated it.
            assert_eq!(
                reported.association_owner.as_deref(),
                Some("amazon"),
                "case {case:?}: expected an AWS-assigned address, got {reported:?}"
            );
            assert!(
                instance
                    .public_dns_name
                    .as_deref()
                    .is_some_and(|name| !name.is_empty()),
                "case {case:?}: expected a public DNS name beside the address, got {instance:?}"
            );
            let read_back =
                described_interface(case, &network.router, &reported.network_interface_id).await;
            assert_eq!(
                read_back.association_public_ip, reported.association_public_ip,
                "case {case:?}: the describe of the interface reports another address: \
                 {read_back:?}"
            );
        } else {
            assert!(
                instance
                    .public_dns_name
                    .as_deref()
                    .is_none_or(str::is_empty),
                "case {case:?}: expected no public DNS name, got {instance:?}"
            );
        }
    }
}

/// THE RULE, undone: an interface a launch created `DeleteOnTermination` goes when its
/// instance reaches `terminated`, and the public IPv4 address AWS assigned it goes with it —
/// while one the launch was told to keep stays, with its addresses.
#[tokio::test]
async fn an_interface_a_launch_created_goes_with_its_instance_only_if_asked() {
    let network = Network::create().await;
    for (case, delete_on_termination, survives) in [
        ("an interface deleted with its instance", "true", false),
        ("an interface kept past its instance", "false", true),
    ] {
        let body = network
            .launch(
                case,
                1,
                &interface(
                    1,
                    &[
                        ("DeviceIndex", "0"),
                        ("SubnetId", network.subnet_a.as_str()),
                        ("DeleteOnTermination", delete_on_termination),
                        ("AssociatePublicIpAddress", "true"),
                    ],
                ),
            )
            .await;
        let instances = launched(case, &body);
        let instance = &instances[0];
        let network_interface_id = instance.interfaces[0].network_interface_id.clone();
        assert!(
            instance.public_ip.is_some(),
            "case {case:?}: expected the public address the launch asked for, got {instance:?}"
        );

        ok(
            case,
            &network.router,
            "TerminateInstances",
            &indexed("InstanceId", &[instance.instance_id.as_str()]),
        )
        .await;
        // The termination's own span, derived from the service's constant rather than from a
        // literal, so the two cannot drift.
        tokio::time::sleep(
            ec2::state::TRANSITION_DELAY + std::time::Duration::from_millis(200),
        )
        .await;

        let read_back = described(case, &network.router, &instance.instance_id).await;
        let interfaces = ok(
            case,
            &network.router,
            "DescribeNetworkInterfaces",
            &super::ec2_query::filters(&[("subnet-id", &[network.subnet_a.as_str()])]),
        )
        .await;
        let still_held = elements(&interfaces, "networkInterfaceId");
        if survives {
            assert!(
                still_held.contains(&network_interface_id),
                "case {case:?}: expected {network_interface_id} to outlive its instance: \
                 {interfaces}"
            );
        } else {
            assert!(
                !still_held.contains(&network_interface_id),
                "case {case:?}: expected {network_interface_id} to go with its instance: \
                 {interfaces}"
            );
            assert!(
                read_back.public_ip.as_deref().is_none_or(str::is_empty),
                "case {case:?}: a terminated instance holds no AWS-assigned public address, \
                 got {read_back:?}"
            );
        }
    }
}

/// One refusal row: the launch's fields, and the status and code EC2 answers.
struct RefusalRow {
    case: &'static str,
    count: u32,
    fields: Vec<Param>,
    status: u16,
    code: &'static str,
}

/// THE RULE, the other half: what a launch asks for that an interface cannot be given is
/// refused, naming the parameter at fault — rather than served with interfaces that are not
/// what was asked for.
#[tokio::test]
async fn a_launch_that_asks_for_what_an_interface_cannot_be_given_is_refused() {
    let network = Network::create().await;
    // One interface already attached to an instance, for the in-use row.
    let attached_interface = {
        let created = ok(
            "the attached interface",
            &network.router,
            "CreateNetworkInterface",
            &[param("SubnetId", &network.subnet_a)],
        )
        .await;
        let network_interface_id = element(&created, "networkInterfaceId").unwrap_or_default();
        network
            .launch(
                "the attached interface",
                1,
                &interface(
                    1,
                    &[
                        ("DeviceIndex", "0"),
                        ("NetworkInterfaceId", &network_interface_id),
                    ],
                ),
            )
            .await;
        network_interface_id
    };
    // One address already held by an instance of the account, for the in-use address row.
    let held_address = {
        let body = network
            .launch(
                "the held address",
                1,
                &interface(
                    1,
                    &[
                        ("DeviceIndex", "0"),
                        ("SubnetId", &network.subnet_a),
                        ("PrivateIpAddress", "10.70.1.99"),
                    ],
                ),
            )
            .await;
        launched("the held address", &body)[0]
            .private_ip
            .clone()
            .unwrap_or_default()
    };
    assert_eq!(
        held_address, "10.70.1.99",
        "the row below needs the address the launch above took"
    );

    let primary = |members: &[(&str, &str)]| interface(1, members);
    let rows = vec![
        RefusalRow {
            case: "an instance-level subnet beside an interface",
            count: 1,
            fields: {
                let mut fields = vec![param("SubnetId", &network.subnet_a)];
                fields.extend(primary(&[
                    ("DeviceIndex", "0"),
                    ("SubnetId", &network.subnet_a),
                ]));
                fields
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            case: "an instance-level security group beside an interface",
            count: 1,
            fields: {
                let mut fields = vec![param("SecurityGroupId.1", &network.group_a)];
                fields.extend(primary(&[
                    ("DeviceIndex", "0"),
                    ("SubnetId", &network.subnet_a),
                ]));
                fields
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            case: "an instance-level security group NAME beside an interface",
            count: 1,
            fields: {
                let mut fields = vec![param("SecurityGroup.1", "interfaces-a")];
                fields.extend(primary(&[
                    ("DeviceIndex", "0"),
                    ("SubnetId", &network.subnet_a),
                ]));
                fields
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            case: "an instance-level private address beside an interface",
            count: 1,
            fields: {
                let mut fields = vec![param("PrivateIpAddress", "10.70.1.12")];
                fields.extend(primary(&[
                    ("DeviceIndex", "0"),
                    ("SubnetId", &network.subnet_a),
                ]));
                fields
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            case: "two interfaces at the same device index",
            count: 1,
            fields: {
                let mut fields = primary(&[("DeviceIndex", "0"), ("SubnetId", &network.subnet_a)]);
                fields.extend(interface(
                    2,
                    &[("DeviceIndex", "0"), ("SubnetId", &network.subnet_b)],
                ));
                fields
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "a device index below zero",
            count: 1,
            fields: primary(&[("DeviceIndex", "-1"), ("SubnetId", &network.subnet_a)]),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "a network card index below zero",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("NetworkCardIndex", "-1"),
            ]),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "no interface at device index 0",
            count: 1,
            fields: primary(&[("DeviceIndex", "1"), ("SubnetId", &network.subnet_a)]),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            // `trunk` is a `NetworkInterfaceCreationType` a create may ask for, and NOT one of
            // the three a launch may: "Valid values: interface | efa | efa-only".
            case: "an interface type outside the launch's published values",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("InterfaceType", "trunk"),
            ]),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an interface type that is no type at all",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("InterfaceType", "not-a-type"),
            ]),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an efa-only interface asking for an address",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("InterfaceType", "efa-only"),
                ("PrivateIpAddress", "10.70.1.31"),
            ]),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            case: "a subnet no account holds",
            count: 1,
            fields: primary(&[("DeviceIndex", "0"), ("SubnetId", UNKNOWN_SUBNET)]),
            status: 400,
            code: "InvalidSubnetID.NotFound",
        },
        RefusalRow {
            case: "a security group no account holds",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("SecurityGroupId.1", UNKNOWN_GROUP),
            ]),
            status: 400,
            code: "InvalidGroup.NotFound",
        },
        RefusalRow {
            case: "a security group of another VPC",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("SecurityGroupId.1", &network.foreign_group),
            ]),
            status: 400,
            code: "InvalidParameter",
        },
        RefusalRow {
            case: "an interface id no account holds",
            count: 1,
            fields: primary(&[("DeviceIndex", "0"), ("NetworkInterfaceId", UNKNOWN_INTERFACE)]),
            status: 400,
            code: "InvalidNetworkInterfaceID.NotFound",
        },
        RefusalRow {
            case: "an interface another instance already holds",
            count: 1,
            fields: primary(&[("DeviceIndex", "0"), ("NetworkInterfaceId", &attached_interface)]),
            status: 400,
            code: "InvalidNetworkInterface.InUse",
        },
        RefusalRow {
            case: "an address outside the subnet's range",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("PrivateIpAddress", "10.70.9.9"),
            ]),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an address another instance already holds",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("PrivateIpAddress", &held_address),
            ]),
            status: 400,
            code: "InvalidIPAddress.InUse",
        },
        RefusalRow {
            case: "an address that is no IPv4 address",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("PrivateIpAddress", "10.70.1.999"),
            ]),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            case: "an address named for a launch of more than one instance",
            count: 2,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("PrivateIpAddress", "10.70.1.61"),
            ]),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            case: "an existing interface named for a launch of more than one instance",
            count: 2,
            fields: primary(&[("DeviceIndex", "0"), ("NetworkInterfaceId", &attached_interface)]),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            case: "delete-on-termination asked of an interface the launch only attaches",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("NetworkInterfaceId", &attached_interface),
                ("DeleteOnTermination", "true"),
            ]),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            case: "a public address asked of an interface the launch only attaches",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("NetworkInterfaceId", &attached_interface),
                ("AssociatePublicIpAddress", "true"),
            ]),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            case: "a public address asked of an interface that is not eth0",
            count: 1,
            fields: {
                let mut fields = primary(&[("DeviceIndex", "0"), ("SubnetId", &network.subnet_a)]);
                fields.extend(interface(
                    2,
                    &[
                        ("DeviceIndex", "1"),
                        ("SubnetId", &network.subnet_b),
                        ("AssociatePublicIpAddress", "true"),
                    ],
                ));
                fields
            },
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalRow {
            case: "an interface in another Availability Zone than the instance",
            count: 1,
            fields: {
                let mut fields = primary(&[("DeviceIndex", "0"), ("SubnetId", &network.subnet_a)]);
                fields.extend(interface(
                    2,
                    &[
                        ("DeviceIndex", "1"),
                        ("SubnetId", &network.subnet_other_zone),
                    ],
                ));
                fields
            },
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            // A device index is a POSITION, so an interface at index 99 would give the
            // instance 100 interfaces — far above what any instance type holds.
            case: "more interfaces than the instance type can hold",
            count: 1,
            fields: {
                let mut fields = primary(&[("DeviceIndex", "0"), ("SubnetId", &network.subnet_a)]);
                fields.extend(interface(
                    2,
                    &[("DeviceIndex", "99"), ("SubnetId", &network.subnet_b)],
                ));
                fields
            },
            status: 400,
            code: "AttachmentLimitExceeded",
        },
        RefusalRow {
            // The list itself is bounded too: `t3.micro` holds two interfaces, so three
            // entries are past its limit however they are indexed.
            case: "a longer list of interfaces than the instance type can hold",
            count: 1,
            fields: {
                let mut fields = vec![param("InstanceType", "t3.micro")];
                fields.extend(primary(&[
                    ("DeviceIndex", "0"),
                    ("SubnetId", &network.subnet_a),
                ]));
                fields.extend(interface(
                    2,
                    &[("DeviceIndex", "1"), ("SubnetId", &network.subnet_b)],
                ));
                fields.extend(interface(
                    3,
                    &[("DeviceIndex", "2"), ("SubnetId", &network.subnet_a)],
                ));
                fields
            },
            status: 400,
            code: "AttachmentLimitExceeded",
        },
        RefusalRow {
            case: "an IPv6 address asked of an interface",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("Ipv6AddressCount", "1"),
            ]),
            status: 501,
            code: "NotImplemented",
        },
        RefusalRow {
            case: "a delegated prefix asked of an interface",
            count: 1,
            fields: primary(&[
                ("DeviceIndex", "0"),
                ("SubnetId", &network.subnet_a),
                ("Ipv4PrefixCount", "1"),
            ]),
            status: 501,
            code: "NotImplemented",
        },
    ];

    for row in &rows {
        let (status, code) = network.refused_launch(row.count, &row.fields).await;
        assert_eq!(
            status, row.status,
            "case {:?}: expected HTTP {}, got {status} with code {code:?}",
            row.case, row.status
        );
        assert_eq!(
            code, row.code,
            "case {:?}: expected the code {:?}, got {code:?}",
            row.case, row.code
        );
    }

    // A refused launch stored nothing: the zone-mismatch row above named a subnet in the
    // other zone, which still holds no interface at all.
    let interfaces_elsewhere = ok(
        "a refused launch stores nothing",
        &network.router,
        "DescribeNetworkInterfaces",
        &super::ec2_query::filters(&[("subnet-id", &[network.subnet_other_zone.as_str()])]),
    )
    .await;
    assert!(
        elements(&interfaces_elsewhere, "networkInterfaceId").is_empty(),
        "expected the subnet in {} to hold no interface: {interfaces_elsewhere}",
        network.zone_b
    );
    assert_eq!(
        network.zone_a,
        ec2::state::availability_zones_in_region(REGION)
            .first()
            .map(|zone| zone.zone_name.clone())
            .unwrap_or_default(),
        "the suite's first zone is the Region's first"
    );
}

/// THE RULE, through a LAUNCH TEMPLATE: a template's `NetworkInterfaces` is the same
/// specification, so the launch creates the interface it describes — and a member the request
/// itself names overwrites the corresponding one on that interface.
#[tokio::test]
async fn a_template_interface_is_created_like_a_requests_own() {
    let network = Network::create().await;
    let case = "a template that names its instance's interface";
    let template = ok(
        case,
        &network.router,
        "CreateLaunchTemplate",
        &[
            param("LaunchTemplateName", "interface-template"),
            param("LaunchTemplateData.ImageId", AMI),
            param("LaunchTemplateData.NetworkInterface.1.DeviceIndex", "0"),
            param(
                "LaunchTemplateData.NetworkInterface.1.SubnetId",
                &network.subnet_a,
            ),
            param(
                "LaunchTemplateData.NetworkInterface.1.SecurityGroupId.1",
                &network.group_a,
            ),
            param(
                "LaunchTemplateData.NetworkInterface.1.Description",
                "from the template",
            ),
            param(
                "LaunchTemplateData.NetworkInterface.1.AssociatePublicIpAddress",
                "true",
            ),
        ],
    )
    .await;
    let launch_template_id = element(&template, "launchTemplateId").unwrap_or_default();

    let body = ok(
        case,
        &network.router,
        "RunInstances",
        &[
            param("MinCount", "1"),
            param("MaxCount", "1"),
            param("LaunchTemplate.LaunchTemplateId", &launch_template_id),
        ],
    )
    .await;
    let instances = launched(case, &body);
    let instance = &instances[0];
    let reported = &instance.interfaces[0];
    assert_eq!(
        reported.subnet_id.as_deref(),
        Some(network.subnet_a.as_str()),
        "case {case:?}: expected the template's subnet, got {reported:?}"
    );
    assert_eq!(
        reported.description.as_deref(),
        Some("from the template"),
        "case {case:?}: expected the template's description, got {reported:?}"
    );
    assert_eq!(
        reported.group_ids,
        vec![network.group_a.clone()],
        "case {case:?}: expected the template's group, got {reported:?}"
    );
    assert!(
        reported.association_public_ip.is_some(),
        "case {case:?}: expected the public address the template asked for, got {reported:?}"
    );

    // "Any additional parameters that you specify for the new instance overwrite the
    // corresponding parameters included in the launch template": the request's own subnet is
    // where the instance's primary interface goes.
    let case = "a request whose subnet overwrites the template interface's";
    let body = ok(
        case,
        &network.router,
        "RunInstances",
        &[
            param("MinCount", "1"),
            param("MaxCount", "1"),
            param("LaunchTemplate.LaunchTemplateId", &launch_template_id),
            param("SubnetId", &network.subnet_b),
        ],
    )
    .await;
    let instances = launched(case, &body);
    let instance = &instances[0];
    let reported = &instance.interfaces[0];
    assert_eq!(
        reported.subnet_id.as_deref(),
        Some(network.subnet_b.as_str()),
        "case {case:?}: expected the request's subnet, got {reported:?}"
    );
    assert_eq!(
        instance.subnet_id.as_deref(),
        Some(network.subnet_b.as_str()),
        "case {case:?}: expected the instance in the request's subnet, got {instance:?}"
    );
}
