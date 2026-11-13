// SPDX-License-Identifier: CC0-1.0

//! What `CreateNatGateway` REPORTS about the addresses of the gateway it just created, and
//! what the clock then owes them, driven over `ec2Query` (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rules under test:
//!
//! 1. a create reports each of the gateway's addresses in the TRANSIENT
//!    `NatGatewayAddressStatus` of the work that create starts — `associating` while an
//!    Elastic IP address is being associated with the gateway's network interface, and
//!    `assigning` while an address that associates none (a private NAT gateway's own
//!    address) is being put on it — and the CLOCK, not a second call, then owes each of them
//!    the terminal `succeeded`. "The address status" (`NatGatewayAddress$Status`) is one of
//!    `assigning | unassigning | associating | disassociating | succeeded | failed`
//!    (`smithy_query(query="shape",shape="NatGatewayAddressStatus")`), so an address being
//!    assigned and one being associated are not the same answer;
//! 2. the Elastic IP address a public NAT gateway needs is required BY THE CONNECTIVITY TYPE
//!    the request asks for — "[Public NAT gateways only] The allocation ID of an Elastic IP
//!    address to associate with the NAT gateway" (`CreateNatGatewayRequest$AllocationId`) —
//!    so a create that leaves it out is refused with EC2's `MissingParameter` naming that
//!    connectivity type and the remedy, rather than with the wording an ordinary absent
//!    parameter gets.
//!
//! The rows of rule 1 differ along what it generalises over: how many Elastic IP addresses
//! the gateway is created with (so the status is asserted per address, not per gateway),
//! whether the connectivity type is asked for or left to its default, and whether the
//! private address is named by the request or chosen by the service. The sibling suite
//! `nat_gateway_lifecycle.rs` covers what a create PERSISTS; this one covers only the status
//! those addresses are reported in and the move the clock owes them.

use std::time::{Duration, Instant};

use ec2::state::{
    CONNECTIVITY_TYPE_PRIVATE, CONNECTIVITY_TYPE_PUBLIC, NAT_GATEWAY_ADDRESS_STATUS_ASSIGNING,
    NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING, NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, error_message};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// EC2's error code for a parameter the action needs and the request left out.
const MISSING_PARAMETER: &str = "MissingParameter";

/// How long the poll below is given before it reports that an address never left its
/// transient status — generously past [`ec2::state::TRANSITION_DELAY`], so a loaded machine
/// fails nothing, while a status the loop's guard never matches still ends the test instead
/// of hanging.
const POLL_DEADLINE: Duration = Duration::from_secs(10);

/// How long the poll waits between reads, as a client's own waiter does.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// A VPC of `cidr_block` with one subnet of `subnet_cidr` in it, both created over the wire,
/// as that subnet's id.
async fn subnet(
    case: &str,
    router: &MultiServiceRouter,
    cidr_block: &str,
    subnet_cidr: &str,
) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    let vpc_id = member(case, &body, "vpcId");
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[
            param("VpcId", &vpc_id),
            param("CidrBlock", subnet_cidr),
            param("AvailabilityZone", "us-east-1a"),
        ],
    )
    .await;
    member(case, &body, "subnetId")
}

/// One Elastic IP address out of Amazon's pool, as its allocation id.
async fn allocation(case: &str, router: &MultiServiceRouter) -> String {
    let body = ok(case, router, "AllocateAddress", &[]).await;
    member(case, &body, "allocationId")
}

/// The `status` of every `natGatewayAddressSet` item in `body`, in document order.
///
/// Read with the XML parser rather than by substring, so a `status` member of any other
/// element of the response can never be read as one of a gateway's addresses.
fn address_statuses(case: &str, body: &str) -> Vec<String> {
    let parsed = super::wire::document(case, body);
    let items: Vec<roxmltree::Node<'_, '_>> = parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "natGatewayAddressSet")
        .flat_map(|set| {
            set.children()
                .filter(|node| node.is_element())
                .collect::<Vec<_>>()
        })
        .collect();
    items
        .into_iter()
        .map(|item| super::wire::child_text(item, "status").unwrap_or_default())
        .collect()
}

/// The statuses `DescribeNatGateways` reports for the addresses of `nat_gateway_id`.
async fn described_statuses(
    case: &str,
    router: &MultiServiceRouter,
    nat_gateway_id: &str,
) -> Vec<String> {
    let body = ok(
        case,
        router,
        "DescribeNatGateways",
        &[param("NatGatewayId.1", nat_gateway_id)],
    )
    .await;
    address_statuses(case, &body)
}

/// Poll `DescribeNatGateways` while any of the gateway's addresses is still in a transient
/// status, exactly as a client waiting for its NAT gateway does, and answer the statuses it
/// finally reported. Nothing but the clock moves them.
async fn settled_statuses(
    case: &str,
    router: &MultiServiceRouter,
    nat_gateway_id: &str,
) -> Vec<String> {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut statuses = described_statuses(case, router, nat_gateway_id).await;
    while statuses
        .iter()
        .any(|status| status != NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED)
        && Instant::now() < deadline
    {
        tokio::time::sleep(POLL_INTERVAL).await;
        statuses = described_statuses(case, router, nat_gateway_id).await;
    }
    statuses
}

// ---------------------------------------------------------------------------
// Rule 1: the transient status a create reports, and the move the clock owes
// ---------------------------------------------------------------------------

/// One row of the create table.
struct CreateCase {
    case: &'static str,
    /// The VPC's block and the subnet's block the gateway is created in.
    network: (&'static str, &'static str),
    /// What the create asks for in `ConnectivityType`, or `None` to leave it out.
    connectivity_type: Option<&'static str>,
    /// How many Elastic IP addresses the create names: the first as `AllocationId`, the rest
    /// as `SecondaryAllocationIds`.
    allocations: usize,
    /// A `PrivateIpAddress` the create names, or `None` to let the service choose one.
    private_ip_address: Option<&'static str>,
    /// The status every address of the created gateway must be reported in, in order.
    expected: &'static [&'static str],
}

/// The rows: a public gateway with one Elastic IP address, one with the two a public gateway
/// may hold, a create that leaves the connectivity type to its documented default ("The
/// default is public connectivity" — `CreateNatGatewayRequest$ConnectivityType`), and a
/// private gateway whose own address is chosen by the service in one row and named by the
/// request in the other.
const CREATE_CASES: &[CreateCase] = &[
    CreateCase {
        case: "a public gateway with one Elastic IP address",
        network: ("10.70.0.0/16", "10.70.1.0/24"),
        connectivity_type: Some(CONNECTIVITY_TYPE_PUBLIC),
        allocations: 1,
        private_ip_address: None,
        expected: &[NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING],
    },
    CreateCase {
        case: "a public gateway with two Elastic IP addresses",
        network: ("10.71.0.0/16", "10.71.1.0/24"),
        connectivity_type: Some(CONNECTIVITY_TYPE_PUBLIC),
        allocations: 2,
        private_ip_address: None,
        expected: &[
            NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING,
            NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING,
        ],
    },
    CreateCase {
        case: "a create that leaves the connectivity type to its default",
        network: ("10.72.0.0/16", "10.72.1.0/24"),
        connectivity_type: None,
        allocations: 1,
        private_ip_address: None,
        expected: &[NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING],
    },
    CreateCase {
        case: "a private gateway whose address the service chooses",
        network: ("10.73.0.0/16", "10.73.1.0/24"),
        connectivity_type: Some(CONNECTIVITY_TYPE_PRIVATE),
        allocations: 0,
        private_ip_address: None,
        expected: &[NAT_GATEWAY_ADDRESS_STATUS_ASSIGNING],
    },
    CreateCase {
        case: "a private gateway whose address the request names",
        network: ("10.74.0.0/16", "10.74.1.0/24"),
        connectivity_type: Some(CONNECTIVITY_TYPE_PRIVATE),
        allocations: 0,
        private_ip_address: Some("10.74.1.100"),
        expected: &[NAT_GATEWAY_ADDRESS_STATUS_ASSIGNING],
    },
];

/// Rule 1: the create answers the transient status of the work it started, and the clock —
/// nothing else — takes every one of those addresses to `succeeded`.
#[tokio::test]
async fn a_created_gateway_reports_its_addresses_settling_and_the_clock_settles_them() {
    for row in CREATE_CASES {
        let case = row.case;
        let router = super::ec2_query::emulator().0;
        let (cidr_block, subnet_cidr) = row.network;
        let subnet_id = subnet(case, &router, cidr_block, subnet_cidr).await;

        let mut params: Vec<Param> = vec![param("SubnetId", &subnet_id)];
        if let Some(connectivity_type) = row.connectivity_type {
            params.push(param("ConnectivityType", connectivity_type));
        }
        for index in 0..row.allocations {
            let allocation_id = allocation(case, &router).await;
            if index == 0 {
                params.push(param("AllocationId", &allocation_id));
            } else {
                params.push(param(
                    &format!("SecondaryAllocationId.{index}"),
                    &allocation_id,
                ));
            }
        }
        if let Some(private_ip_address) = row.private_ip_address {
            params.push(param("PrivateIpAddress", private_ip_address));
        }

        let created = ok(case, &router, "CreateNatGateway", &params).await;
        let reported = address_statuses(case, &created);
        assert_eq!(
            reported,
            super::ec2_query::owned(row.expected),
            "case {case:?}: expected the create to report {:?}, got {reported:?} in {created}",
            row.expected
        );

        // The very next read reports the SAME transient statuses: the create answered what it
        // stored, so a client polling straight after it still sees the work in progress.
        let nat_gateway_id = member(case, &created, "natGatewayId");
        let straight_after = described_statuses(case, &router, &nat_gateway_id).await;
        assert_eq!(
            straight_after,
            super::ec2_query::owned(row.expected),
            "case {case:?}: expected the read straight after the create to report {:?}, got \
             {straight_after:?}",
            row.expected
        );

        // And the clock finishes the work: every address reaches `succeeded`, whether it was
        // associating an Elastic IP address or assigning a private one, with nothing but the
        // poll's reads in between.
        let settled = settled_statuses(case, &router, &nat_gateway_id).await;
        assert_eq!(
            settled,
            vec![NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED.to_string(); row.expected.len()],
            "case {case:?}: expected every address to reach \
             {NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED}, got {settled:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the refusal names the connectivity type the address is required for
// ---------------------------------------------------------------------------

/// Rule 2: a create that needs an Elastic IP address for the connectivity type it asks for
/// is refused `MissingParameter` with the connectivity type in the message — whether that
/// type was asked for or left to its default, which is the only part of the message the
/// request decides.
#[tokio::test]
async fn a_create_without_the_address_its_connectivity_type_needs_names_that_type() {
    // One row per way the request can ask for public connectivity: by naming it, and by
    // leaving `ConnectivityType` out ("The default is public connectivity").
    for (case, connectivity_type) in [
        (
            "a public gateway with no Elastic IP address",
            Some(CONNECTIVITY_TYPE_PUBLIC),
        ),
        ("a create that names no connectivity type at all", None),
    ] {
        let router = super::ec2_query::emulator().0;
        let subnet_id = subnet(case, &router, "10.75.0.0/16", "10.75.1.0/24").await;
        let mut params: Vec<Param> = vec![param("SubnetId", &subnet_id)];
        if let Some(connectivity_type) = connectivity_type {
            params.push(param("ConnectivityType", connectivity_type));
        }

        let (status, refused) = call(&router, REGION, "CreateNatGateway", &params).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, MISSING_PARAMETER),
            "case {case:?}: expected HTTP 400 {MISSING_PARAMETER}, got {status} with body \
             {refused}"
        );
        // The connectivity type the address is required for is the one the request asked
        // for, which is `public` whether it named it or left it out.
        let asked_for = connectivity_type.unwrap_or(CONNECTIVITY_TYPE_PUBLIC);
        let expected = format!(
            "AllocationId is required for a NAT Gateway with Connectivity Type {asked_for}. \
             Please provide the AllocationId and retry the request."
        );
        let reported = error_message(&refused);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the message {expected:?}, got {reported:?}"
        );

        // A refused create stores nothing: there is no gateway to report.
        let listed = ok(case, &router, "DescribeNatGateways", &[]).await;
        assert_eq!(
            address_statuses(case, &listed),
            Vec::<String>::new(),
            "case {case:?}: expected the refused create to store no gateway, got {listed}"
        );
    }
}
