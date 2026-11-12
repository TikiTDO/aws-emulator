// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetNetworkRoutes`, driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: a read of a global network's routes answers the ONE route table its
//! single `RouteTableIdentifier` member addresses — its ARN, its type and the instant it came
//! into being — and the routes that table holds, each computed from what this service stores:
//! the static routes the policy in force declares for a segment (every prefix of a
//! `create-route` action, reached through each destination that is an available attachment of
//! that table, and reached through NOTHING — a blackhole route — where none of them is) and
//! the prefixes the table's BGP peerings carry (each inside CIDR block of a Connect peer of
//! an attachment of that table, propagated into it). One route per prefix: a prefix two
//! actions declare is held once, reached through what both name, and a prefix the policy
//! declares statically is held as that static route rather than as the propagated one of the
//! same block. Every match the request carries narrows the answer and they combine. A segment
//! or group the policy in force does not declare, and a Region it configures no edge in,
//! address no route table and are refused on the member that carried them; an unknown global
//! network or core network, and a transit gateway route table no attachment of this global
//! network attaches, are not found.
//!
//! The tables below span that class: rows differing in the FORM the route table is addressed
//! by (a core network segment edge, a network function group edge, a transit gateway route
//! table), in the segment (one present at every edge, one restricted to a single edge), in
//! the edge location, in the kind of route (static to one attachment, static to several,
//! static to an attachment of another edge, blackholed by the policy itself, static to an
//! attachment nothing holds, learned over a peering), in the address family of the prefix, in
//! the kind of resource a destination reaches (a VPC, a Connect attachment, a transit gateway
//! route table reached over a peering, a Connect peer) and — for the matches — in which
//! attribute is matched, on how many values, in how a CIDR is spelled, and in how many
//! matches combine.
//!
//! The policy document's own key names (`segment-actions`, `action`, `create-route`,
//! `destination-cidr-blocks`, `destinations`, `blackhole`, `network-function-groups`,
//! `attachment-policies`, `association-method`, `tag-value-of-key`,
//! `add-to-network-function-group`) are the published Cloud WAN core network policy schema's,
//! the same source the other suites build documents from.

use std::future::Future;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_networkmanager_networkmanager::state;

use crate::attachment_lifecycle::{
    attachment_id, create_connect_attachment, create_vpc_attachment, subnet_arn, vpc_arn,
};
use crate::connect_peer_lifecycle::create_connect_peer;
use crate::core_network_lifecycle::create_core_network;
use crate::core_network_policy_versions::get_policy;
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::resource_tagging::tag;

/// The two edge locations the subject's core network configures.
const WEST: &str = "us-west-2";
/// See [`WEST`].
const CENTRAL: &str = "eu-central-1";

/// The segments the subject's policy declares: one present at every edge, and one restricted
/// to the western edge.
const SHARED: &str = "shared";
/// See [`SHARED`].
const PROD: &str = "prod";

/// The network function group the subject's policy declares, present at every edge.
const INSPECTION: &str = "inspection";

/// The tag keys the subject's attachment-policy rules take the segment and the network
/// function group of an attachment from.
const SEGMENT_TAG: &str = "segment";
/// See [`SEGMENT_TAG`].
const GROUP_TAG: &str = "group";

/// The appliance addresses the subject's two BGP peerings carry.
const WEST_PEER_ADDRESS: &str = "203.0.113.1";
/// See [`WEST_PEER_ADDRESS`].
const INSPECTION_PEER_ADDRESS: &str = "203.0.113.9";

/// The inside IP addresses the western peering runs BGP over: an IPv4 and an IPv6 block, so
/// the prefixes a peering propagates are exercised in both address families. The IPv4 one is
/// also declared as a STATIC route by the subject's policy, which is what pins that a prefix
/// the policy declares is held as the static route rather than as the propagated one.
const WEST_INSIDE_CIDR_BLOCKS: [&str; 2] = ["169.254.10.0/29", "fd00::/125"];
/// See [`WEST_INSIDE_CIDR_BLOCKS`]: the block the peering of the network function group's
/// Connect attachment propagates.
const INSPECTION_INSIDE_CIDR_BLOCK: &str = "169.254.20.0/29";

/// An attachment id of the shape the model fixes (`^attachment-([0-9a-f]{8,17})$`,
/// `smithy_query shape=AttachmentId`) that no attachment of the subject carries: what a
/// `create-route` destination naming nothing stored looks like.
const ABSENT_ATTACHMENT: &str = "attachment-0123456789abcdef0";

/// A managed prefix list id, as the id of such a list reads (`prefix-list/pl-<hex>`, the form
/// `PrefixListArn`'s documentation gives): the value the `PrefixListIds` match carries, which
/// no route of this service holds a prefix list for.
const PREFIX_LIST_ID: &str = "pl-0123456789abcdef1";

/// The `RouteTableType` each form of the identifier answers (`smithy_query
/// shape=RouteTableType`).
const CORE_NETWORK_SEGMENT: &str = "CORE_NETWORK_SEGMENT";
/// See [`CORE_NETWORK_SEGMENT`].
const NETWORK_FUNCTION_GROUP: &str = "NETWORK_FUNCTION_GROUP";
/// See [`CORE_NETWORK_SEGMENT`].
const TRANSIT_GATEWAY_ROUTE_TABLE: &str = "TRANSIT_GATEWAY_ROUTE_TABLE";

/// The two values `RouteState` enumerates and the two `RouteType` does (`smithy_query
/// shape=RouteState,RouteType`).
const ACTIVE: &str = "ACTIVE";
/// See [`ACTIVE`].
const BLACKHOLE: &str = "BLACKHOLE";
/// See [`ACTIVE`].
const STATIC: &str = "STATIC";
/// See [`ACTIVE`].
const PROPAGATED: &str = "PROPAGATED";

/// How long a wait for a transition the clock owes may take before the suite gives up, and
/// how long it waits between looks.
///
/// Every wait here is on a CONDITION — a created resource reaching the state the clock owes
/// it — so the suite looks again until that state holds rather than sleeping for a span: the
/// deadline is generous, so a loaded or a faster host changes nothing about what is asserted.
const DEADLINE: Duration = Duration::from_secs(60);
/// See [`DEADLINE`].
const LOOK_AGAIN: Duration = Duration::from_millis(20);

/// Look again until `holds` answers true, or [`DEADLINE`] passes.
async fn poll_until<Look, Held>(what: &str, mut holds: Look)
where
    Look: FnMut() -> Held,
    Held: Future<Output = bool>,
{
    let started = Instant::now();
    loop {
        if holds().await {
            return;
        }
        assert!(
            started.elapsed() < DEADLINE,
            "case {what:?}: the condition did not hold within {DEADLINE:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One attachment of the subject: its id and the ARN of the resource it attaches, which is
/// what a destination reaching it reports the resource by.
struct Attached {
    id: String,
    resource_arn: String,
}

/// The subject: a core network with two edges, two segments and one network function group,
/// the attachments of each of them, the BGP peering of each Connect attachment, and the
/// transit gateway route table attached over a peering.
struct Subject {
    global_network: String,
    core_network: String,
    core_network_arn: String,
    /// The VPC attachment of [`SHARED`] at [`WEST`].
    west_vpc: Attached,
    /// The VPC attachment of [`SHARED`] at [`CENTRAL`].
    central_vpc: Attached,
    /// The VPC attachment of [`PROD`] at [`WEST`].
    prod_vpc: Attached,
    /// The Connect attachment of [`SHARED`] at [`WEST`], carried over [`Subject::west_vpc`].
    west_connect: Attached,
    /// The Connect attachment of [`INSPECTION`] at [`WEST`].
    inspection_connect: Attached,
    /// The transit gateway route table attachment of [`SHARED`] at [`WEST`], and the ARN of
    /// the route table it attaches.
    route_table_attachment: Attached,
    transit_gateway_route_table_arn: String,
    /// The id the peering that attachment was created over left on the transit gateway side,
    /// which is the transit gateway attachment a destination reaching it is reached through.
    transit_gateway_attachment_id: String,
    /// The Connect peers: the one of [`SHARED`] at [`WEST`] and the one of [`INSPECTION`].
    west_peer: String,
    inspection_peer: String,
}

/// The policy document the subject's core network holds, carrying `routes` as its
/// `segment-actions`.
///
/// Every attachment joins the segment its own `segment` tag names, or the network function
/// group its `group` tag names — the `association-method: tag` and
/// `add-to-network-function-group` forms of an attachment-policy rule's action — so one
/// document serves the attachments of both segments and of the group. The group's rule is
/// numbered BELOW the segment rule, so it is the one that claims an attachment carrying the
/// group tag.
fn policy_document(routes: &Value) -> String {
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": [
                { "location": WEST, "asn": 64512 },
                { "location": CENTRAL, "asn": 64513 },
            ],
        },
        "segments": [
            { "name": SHARED },
            { "name": PROD, "edge-locations": [WEST] },
        ],
        "network-function-groups": [
            { "name": INSPECTION, "require-attachment-acceptance": false },
        ],
        "attachment-policies": [
            {
                "rule-number": 50,
                "conditions": [{
                    "type": "tag-exists",
                    "key": GROUP_TAG,
                }],
                "action": { "add-to-network-function-group": INSPECTION },
            },
            {
                "rule-number": 100,
                "conditions": [{ "type": "any" }],
                "action": { "association-method": "tag", "tag-value-of-key": SEGMENT_TAG },
            },
        ],
        "segment-actions": routes,
    })
    .to_string()
}

/// The `segment-actions` the subject's policy declares, naming the attachments the subject
/// holds: a prefix reached through two attachments named by two separate actions, two
/// prefixes of one action, a prefix reached through an attachment of the other edge AND
/// through nothing, a prefix reached through the transit gateway route table, a prefix the
/// western peering also carries, a prefix whose destination names no stored attachment, a
/// less specific prefix that overlaps a more specific one, and one action acting on a segment
/// the document never declares.
fn subject_routes(subject: &Subject) -> Value {
    json!([
        {
            "action": "create-route",
            "segment": SHARED,
            "destination-cidr-blocks": ["10.0.0.0/16", "10.1.0.0/16"],
            "destinations": [subject.west_vpc.id],
        },
        {
            "action": "create-route",
            "segment": SHARED,
            "destination-cidr-blocks": ["10.0.0.0/16"],
            "destinations": [subject.west_connect.id],
        },
        {
            "action": "create-route",
            "segment": SHARED,
            "destination-cidr-blocks": ["10.0.0.0/8"],
            "destinations": [subject.west_vpc.id],
        },
        {
            "action": "create-route",
            "segment": SHARED,
            "destination-cidr-blocks": ["10.2.0.0/16"],
            "destinations": [subject.central_vpc.id, "blackhole"],
        },
        {
            "action": "create-route",
            "segment": SHARED,
            "destination-cidr-blocks": ["10.3.0.0/16"],
            "destinations": [subject.route_table_attachment.id],
        },
        {
            "action": "create-route",
            "segment": SHARED,
            "destination-cidr-blocks": [WEST_INSIDE_CIDR_BLOCKS[0]],
            "destinations": [subject.west_vpc.id],
        },
        {
            "action": "create-route",
            "segment": SHARED,
            "destination-cidr-blocks": ["192.168.0.0/24"],
            "destinations": [ABSENT_ATTACHMENT],
        },
        {
            "action": "create-route",
            "segment": PROD,
            "destination-cidr-blocks": ["172.16.0.0/16"],
            "destinations": [subject.prod_vpc.id],
        },
        {
            "action": "create-route",
            "segment": "undeclared",
            "destination-cidr-blocks": ["203.0.113.0/24"],
            "destinations": ["blackhole"],
        },
    ])
}

/// An EC2 transit gateway ARN in `region`, in the form the member's own documentation gives
/// (`smithy_query query="doc" shape="CreateTransitGatewayPeeringRequest"
/// member="TransitGatewayArn"`).
fn transit_gateway_arn(region: &str) -> String {
    format!("arn:aws:ec2:{region}:123456789012:transit-gateway/tgw-0123abcd")
}

/// A transit gateway ROUTE TABLE ARN in `region`, in the documented form of
/// `TransitGatewayRouteTableArn` (`arn:aws:ec2:us-west-2:123456789012:
/// transit-gateway-route-table/tgw-rtb-9876543210123456`).
fn transit_gateway_route_table_arn(region: &str, table: &str) -> String {
    format!("arn:aws:ec2:{region}:123456789012:transit-gateway-route-table/tgw-rtb-{table}")
}

/// The attachment a create answered: its id and the ARN of the resource it attaches, both
/// read back out of the envelope every attachment kind shares.
fn attached(case: &str, wrapper: &Value) -> Attached {
    Attached {
        id: attachment_id(case, wrapper),
        resource_arn: wrapper["Attachment"]["ResourceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Attachment.ResourceArn in {wrapper}"))
            .to_string(),
    }
}

/// `CreateVpcAttachment` of the VPC `index` in `region`, carrying `tags`, asserting the
/// create succeeded.
async fn vpc_attachment(
    emulator: &Emulator,
    core_network: &str,
    region: &str,
    index: usize,
    tags: Value,
) -> Attached {
    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "VpcArn": vpc_arn(region, index),
            "SubnetArns": [subnet_arn(region, index)],
            "Tags": tags,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the VPC attachment {index} at {region}: CreateVpcAttachment expected 200, \
         got {status} with {created}"
    );
    attached("vpc attachment", &created["VpcAttachment"])
}

/// `CreateConnectAttachment` over `transport` at `region`, carrying `tags`, asserting the
/// create succeeded.
async fn connect_attachment(
    emulator: &Emulator,
    core_network: &str,
    region: &str,
    transport: &str,
    tags: Value,
) -> Attached {
    let (status, created) = create_connect_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network,
            "EdgeLocation": region,
            "TransportAttachmentId": transport,
            "Options": { "Protocol": "GRE" },
            "Tags": tags,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the Connect attachment over {transport}: CreateConnectAttachment expected 200, \
         got {status} with {created}"
    );
    attached("connect attachment", &created["ConnectAttachment"])
}

/// `CreateConnectPeer` on `attachment`, asserting the create succeeded and answering the
/// minted peer id.
async fn connect_peer(
    emulator: &Emulator,
    attachment: &str,
    address: &str,
    inside_cidr_blocks: &[&str],
) -> String {
    let (status, created) = create_connect_peer(
        emulator,
        json!({
            "ConnectAttachmentId": attachment,
            "PeerAddress": address,
            "CoreNetworkAddress": "203.0.113.254",
            "InsideCidrBlocks": inside_cidr_blocks,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the peering at {address}: CreateConnectPeer expected 200, got {status} with {created}"
    );
    created["ConnectPeer"]["ConnectPeerId"]
        .as_str()
        .unwrap_or_else(|| panic!("the peering at {address}: no ConnectPeerId in {created}"))
        .to_string()
}

/// Peer a transit gateway in `region` with `core_network` and attach the route table
/// `table` of that gateway over the peering, answering the attachment, the route table's ARN
/// and the id the peering left on the gateway side.
async fn route_table_attachment(
    emulator: &Emulator,
    core_network: &str,
    region: &str,
    table: &str,
    tags: Value,
) -> (Attached, String, String) {
    let (status, peered) = emulator
        .post(
            "/transit-gateway-peerings",
            json!({
                "CoreNetworkId": core_network,
                "TransitGatewayArn": transit_gateway_arn(region),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateTransitGatewayPeering expected 200, got {status} with {peered}"
    );
    let peering = peered["TransitGatewayPeering"]["Peering"]["PeeringId"]
        .as_str()
        .unwrap_or_else(|| panic!("no Peering.PeeringId in {peered}"))
        .to_string();
    let transit_gateway_attachment_id = peered["TransitGatewayPeering"]
        ["TransitGatewayPeeringAttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("no TransitGatewayPeeringAttachmentId in {peered}"))
        .to_string();

    let arn = transit_gateway_route_table_arn(region, table);
    let (status, created) = emulator
        .post(
            "/transit-gateway-route-table-attachments",
            json!({
                "PeeringId": peering,
                "TransitGatewayRouteTableArn": arn,
                "Tags": tags,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateTransitGatewayRouteTableAttachment expected 200, got {status} with {created}"
    );
    (
        attached(
            "transit gateway route table attachment",
            &created["TransitGatewayRouteTableAttachment"],
        ),
        arn,
        transit_gateway_attachment_id,
    )
}

/// The tag list an attachment of `segment` carries, which is what the subject's
/// attachment-policy rule puts it in that segment by.
fn segment_tags(segment: &str) -> Value {
    json!([tag(SEGMENT_TAG, segment)])
}

/// Put `document` in force on `core_network`, as its next executed policy version.
///
/// A policy document is put IN FORCE by the create of a core network and by
/// `ExecuteCoreNetworkChangeSet` executing the change set a `PutCoreNetworkPolicy` prepared,
/// so this drives both: this suite needs a SECOND version to take effect, because a
/// `create-route` action names the attachments it reaches by id and those exist only after
/// the core network does. The change set is executed once it is READY, which is the state the
/// clock owes it after a put — looked for rather than waited out.
async fn put_policy_in_force(emulator: &Emulator, core_network: &str, document: String) {
    let (status, put) = emulator
        .post(
            &format!("/core-networks/{core_network}/core-network-policy"),
            json!({ "PolicyDocument": document }),
        )
        .await;
    assert_eq!(
        status, 200,
        "PutCoreNetworkPolicy expected 200, got {status} with {put}"
    );
    let version = put["CoreNetworkPolicy"]["PolicyVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("no CoreNetworkPolicy.PolicyVersionId in {put}"));

    poll_until("the change set is ready to execute", || async {
        let (status, read) = get_policy(
            emulator,
            core_network,
            &format!("?policyVersionId={version}"),
        )
        .await;
        status == 200
            && read["CoreNetworkPolicy"]["ChangeSetState"] == json!("READY_TO_EXECUTE")
    })
    .await;

    let (status, executed) = emulator
        .post(
            &format!("/core-networks/{core_network}/core-network-change-sets/{version}/execute"),
            json!({}),
        )
        .await;
    assert_eq!(
        status, 200,
        "ExecuteCoreNetworkChangeSet expected 200, got {status} with {executed}"
    );
}

/// Build the subject, then put in force a policy whose `create-route` actions name the
/// attachments it has just created, and wait for every attachment and peering to reach the
/// state the clock owes it — a route is held only through a destination that is AVAILABLE.
async fn subject(emulator: &Emulator) -> Subject {
    let global_network = emulator.create_global_network("network routes").await;
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": global_network,
            "Description": "network routes",
            "PolicyDocument": policy_document(&json!([])),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork expected 200, got {status} with {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkId in {created}"))
        .to_string();
    let core_network_arn = created["CoreNetwork"]["CoreNetworkArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkArn in {created}"))
        .to_string();

    let west_vpc = vpc_attachment(emulator, &core_network, WEST, 1, segment_tags(SHARED)).await;
    let central_vpc =
        vpc_attachment(emulator, &core_network, CENTRAL, 2, segment_tags(SHARED)).await;
    let prod_vpc = vpc_attachment(emulator, &core_network, WEST, 3, segment_tags(PROD)).await;
    let inspection_vpc = vpc_attachment(
        emulator,
        &core_network,
        WEST,
        4,
        json!([tag(GROUP_TAG, INSPECTION)]),
    )
    .await;
    let west_connect = connect_attachment(
        emulator,
        &core_network,
        WEST,
        &west_vpc.id,
        segment_tags(SHARED),
    )
    .await;
    let inspection_connect = connect_attachment(
        emulator,
        &core_network,
        WEST,
        &inspection_vpc.id,
        json!([tag(GROUP_TAG, INSPECTION)]),
    )
    .await;
    let west_peer = connect_peer(
        emulator,
        &west_connect.id,
        WEST_PEER_ADDRESS,
        &WEST_INSIDE_CIDR_BLOCKS,
    )
    .await;
    let inspection_peer = connect_peer(
        emulator,
        &inspection_connect.id,
        INSPECTION_PEER_ADDRESS,
        &[INSPECTION_INSIDE_CIDR_BLOCK],
    )
    .await;
    let (route_table_attachment, transit_gateway_route_table_arn, transit_gateway_attachment_id) =
        route_table_attachment(
            emulator,
            &core_network,
            WEST,
            "9876543210123456",
            segment_tags(SHARED),
        )
        .await;

    let subject = Subject {
        global_network,
        core_network,
        core_network_arn,
        west_vpc,
        central_vpc,
        prod_vpc,
        west_connect,
        inspection_connect,
        route_table_attachment,
        transit_gateway_route_table_arn,
        transit_gateway_attachment_id,
        west_peer,
        inspection_peer,
    };
    put_policy_in_force(
        emulator,
        &subject.core_network,
        policy_document(&subject_routes(&subject)),
    )
    .await;

    // Every attachment and every peering owes the clock its `CREATING` -> `AVAILABLE`
    // transition, and a destination reaches a prefix only once it is available, so the suite
    // looks until every one of them has made it.
    let attachments = [
        subject.west_vpc.id.clone(),
        subject.central_vpc.id.clone(),
        subject.prod_vpc.id.clone(),
        inspection_vpc.id.clone(),
        subject.west_connect.id.clone(),
        subject.inspection_connect.id.clone(),
        subject.route_table_attachment.id.clone(),
    ];
    poll_until("every attachment is available", || async {
        let (status, listed) = emulator
            .get(&format!(
                "/attachments?coreNetworkId={}",
                subject.core_network
            ))
            .await;
        status == 200
            && attachments.iter().all(|id| {
                listed["Attachments"]
                    .as_array()
                    .is_some_and(|held| {
                        held.iter().any(|attachment| {
                            attachment["AttachmentId"] == json!(id)
                                && attachment["State"] == json!("AVAILABLE")
                        })
                    })
            })
    })
    .await;
    for peer in [&subject.west_peer, &subject.inspection_peer] {
        poll_until("the peering is available", || async {
            let (status, read) = emulator.get(&format!("/connect-peers/{peer}")).await;
            status == 200 && read["ConnectPeer"]["State"] == json!("AVAILABLE")
        })
        .await;
    }

    subject
}

/// The `RouteTableIdentifier` naming the segment `segment` of `core_network` at `edge`.
fn segment_edge(core_network: &str, segment: &str, edge: &str) -> Value {
    json!({
        "CoreNetworkSegmentEdge": {
            "CoreNetworkId": core_network,
            "SegmentName": segment,
            "EdgeLocation": edge,
        }
    })
}

/// The `RouteTableIdentifier` naming the network function group `group` of `core_network` at
/// `edge`.
fn group_edge(core_network: &str, group: &str, edge: &str) -> Value {
    json!({
        "CoreNetworkNetworkFunctionGroup": {
            "CoreNetworkId": core_network,
            "NetworkFunctionGroupName": group,
            "EdgeLocation": edge,
        }
    })
}

/// `GetNetworkRoutes` on `global_network` for the route table `identifier` names, plus
/// whatever `matches` the request carries.
async fn get_routes(
    emulator: &Emulator,
    global_network: &str,
    identifier: &Value,
    matches: &Value,
) -> (u16, Value) {
    let mut body = json!({ "RouteTableIdentifier": identifier });
    if let Some(members) = matches.as_object() {
        for (name, value) in members {
            body[name] = value.clone();
        }
    }
    emulator
        .post(&format!("/global-networks/{global_network}/network-routes"), body)
        .await
}

/// A successful read, asserting the call answered 200.
async fn read(
    emulator: &Emulator,
    case: &str,
    global_network: &str,
    identifier: &Value,
    matches: &Value,
) -> Value {
    let (status, body) = get_routes(emulator, global_network, identifier, matches).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetNetworkRoutes expected 200, got {status} with {body}"
    );
    body
}

/// The routes one answer reports, in the order returned.
fn routes_of(case: &str, body: &Value) -> Vec<Value> {
    body["NetworkRoutes"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a NetworkRoutes array in {body}"))
        .clone()
}

/// The prefixes one answer reports, in the order returned.
fn prefixes_of(case: &str, body: &Value) -> Vec<String> {
    routes_of(case, body)
        .iter()
        .map(|route| {
            route["DestinationCidrBlock"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a route carries no prefix: {route}"))
                .to_string()
        })
        .collect()
}

/// The destination reaching the attachment `reached`, which the route reports the attachment
/// by, the resource it attaches as that resource's own ARN names it, and the segment or group
/// the policy put it in.
fn destination(reached: &Attached, segment: Option<&str>, group: Option<&str>, edge: &str) -> Value {
    let mut entry = json!({
        "CoreNetworkAttachmentId": reached.id,
        "EdgeLocation": edge,
        "ResourceId": state::arn_resource_id(&reached.resource_arn),
        "ResourceType": state::arn_resource_kind(&reached.resource_arn)
            .expect("the ARN of an attached resource names its type"),
    });
    if let Some(name) = segment {
        entry["SegmentName"] = json!(name);
    }
    if let Some(name) = group {
        entry["NetworkFunctionGroupName"] = json!(name);
    }
    entry
}

/// The destination reaching a route table of a transit gateway: the attachment that attaches
/// it, plus the attachment the peering it is reached over left on the gateway side.
fn gateway_destination(subject: &Subject) -> Value {
    let mut entry = destination(
        &subject.route_table_attachment,
        Some(SHARED),
        None,
        WEST,
    );
    entry["TransitGatewayAttachmentId"] = json!(subject.transit_gateway_attachment_id);
    entry
}

/// The destination reaching a prefix learned over a BGP peering: the Connect attachment
/// carrying the peering, and the peering itself as the resource reached.
fn peer_destination(
    peer: &str,
    attachment: &Attached,
    segment: Option<&str>,
    group: Option<&str>,
) -> Value {
    let mut entry = json!({
        "CoreNetworkAttachmentId": attachment.id,
        "EdgeLocation": WEST,
        "ResourceId": peer,
        "ResourceType": "connect-peer",
    });
    if let Some(name) = segment {
        entry["SegmentName"] = json!(name);
    }
    if let Some(name) = group {
        entry["NetworkFunctionGroupName"] = json!(name);
    }
    entry
}

/// One reported route: its prefix, its state, its type and the destinations it is reached
/// through.
fn route(prefix: &str, state: &str, route_type: &str, destinations: Vec<Value>) -> Value {
    json!({
        "DestinationCidrBlock": prefix,
        "State": state,
        "Type": route_type,
        "Destinations": destinations,
    })
}

/// A blackholed route: one the policy declares and nothing reaches.
fn blackholed(prefix: &str) -> Value {
    route(prefix, BLACKHOLE, STATIC, Vec::new())
}

/// The routes [`SHARED`] holds at [`WEST`], in the order the answer reports them: ascending
/// prefix, which is the key the table holds one route per.
fn west_shared(subject: &Subject) -> Vec<Value> {
    let vpc = || destination(&subject.west_vpc, Some(SHARED), None, WEST);
    vec![
        // The prefix two separate actions declare: held ONCE, reached through what both of
        // them name, in the order the document declares them.
        route(
            "10.0.0.0/16",
            ACTIVE,
            STATIC,
            vec![
                vpc(),
                destination(&subject.west_connect, Some(SHARED), None, WEST),
            ],
        ),
        // The less specific prefix overlapping it, which is what a longest-prefix match has
        // to choose between. The answer is in PREFIX TEXT order, which is why it follows the
        // /16 of the same block.
        route("10.0.0.0/8", ACTIVE, STATIC, vec![vpc()]),
        // The second prefix of the first action.
        route("10.1.0.0/16", ACTIVE, STATIC, vec![vpc()]),
        // The action naming the attachment of the OTHER edge and `blackhole`: nothing it
        // names is an attachment of this segment at this edge, so the route is blackholed.
        blackholed("10.2.0.0/16"),
        // The prefix reached through the transit gateway route table, over the peering that
        // route table is attached by.
        route(
            "10.3.0.0/16",
            ACTIVE,
            STATIC,
            vec![gateway_destination(subject)],
        ),
        // The block the western peering carries AND the policy declares statically: held as
        // the static route, not as a second route for the same block.
        route(
            WEST_INSIDE_CIDR_BLOCKS[0],
            ACTIVE,
            STATIC,
            vec![vpc()],
        ),
        // The action naming an attachment nothing holds.
        blackholed("192.168.0.0/24"),
        // The other block the western peering carries, propagated into the table.
        route(
            WEST_INSIDE_CIDR_BLOCKS[1],
            ACTIVE,
            PROPAGATED,
            vec![peer_destination(
                &subject.west_peer,
                &subject.west_connect,
                Some(SHARED),
                None,
            )],
        ),
    ]
}

/// One row of the route-table table: the identifier a read is addressed by, and what the
/// answer must carry.
struct TableCase {
    /// Names the case in every assertion message.
    name: &'static str,
    identifier: Value,
    /// `RouteTableType`.
    kind: &'static str,
    /// `RouteTableArn`.
    arn: String,
    /// `CoreNetworkSegmentEdge`, `Value::Null` for a table the answer reports none for.
    segment_edge: Value,
    /// Whether `RouteTableTimestamp` is reported: a core network's route tables came into
    /// being when the policy declaring them came into force, which this service knows; a
    /// transit gateway's route table was created by that gateway, which it does not.
    timestamped: bool,
    routes: Vec<Value>,
}

/// The table: one row per route table the subject can be asked about.
fn table_cases(subject: &Subject) -> Vec<TableCase> {
    let core_network = subject.core_network.as_str();
    vec![
        TableCase {
            name: "the segment at every edge, at the western one",
            identifier: segment_edge(core_network, SHARED, WEST),
            kind: CORE_NETWORK_SEGMENT,
            arn: subject.core_network_arn.clone(),
            segment_edge: json!({
                "CoreNetworkId": core_network,
                "SegmentName": SHARED,
                "EdgeLocation": WEST,
            }),
            timestamped: true,
            routes: west_shared(subject),
        },
        TableCase {
            name: "the same segment at the central edge",
            identifier: segment_edge(core_network, SHARED, CENTRAL),
            kind: CORE_NETWORK_SEGMENT,
            arn: subject.core_network_arn.clone(),
            segment_edge: json!({
                "CoreNetworkId": core_network,
                "SegmentName": SHARED,
                "EdgeLocation": CENTRAL,
            }),
            timestamped: true,
            // Every prefix the policy declares for the segment is in its table at this edge
            // too, but only the one naming the attachment THIS edge holds is reached: the
            // others are blackholed here. The peering sits at the other edge, so nothing is
            // propagated into this table.
            routes: vec![
                blackholed("10.0.0.0/16"),
                blackholed("10.0.0.0/8"),
                blackholed("10.1.0.0/16"),
                route(
                    "10.2.0.0/16",
                    ACTIVE,
                    STATIC,
                    vec![destination(
                        &subject.central_vpc,
                        Some(SHARED),
                        None,
                        CENTRAL,
                    )],
                ),
                blackholed("10.3.0.0/16"),
                blackholed(WEST_INSIDE_CIDR_BLOCKS[0]),
                blackholed("192.168.0.0/24"),
            ],
        },
        TableCase {
            name: "the segment restricted to the western edge, at that edge",
            identifier: segment_edge(core_network, PROD, WEST),
            kind: CORE_NETWORK_SEGMENT,
            arn: subject.core_network_arn.clone(),
            segment_edge: json!({
                "CoreNetworkId": core_network,
                "SegmentName": PROD,
                "EdgeLocation": WEST,
            }),
            timestamped: true,
            routes: vec![route(
                "172.16.0.0/16",
                ACTIVE,
                STATIC,
                vec![destination(&subject.prod_vpc, Some(PROD), None, WEST)],
            )],
        },
        TableCase {
            name: "that segment at an edge it is not present at",
            identifier: segment_edge(core_network, PROD, CENTRAL),
            kind: CORE_NETWORK_SEGMENT,
            arn: subject.core_network_arn.clone(),
            segment_edge: json!({
                "CoreNetworkId": core_network,
                "SegmentName": PROD,
                "EdgeLocation": CENTRAL,
            }),
            timestamped: true,
            // The segment restricts itself to the other edge, so it has no route table here
            // and holds nothing, however much it holds there.
            routes: Vec::new(),
        },
        TableCase {
            name: "the network function group at the western edge",
            identifier: group_edge(core_network, INSPECTION, WEST),
            kind: NETWORK_FUNCTION_GROUP,
            arn: subject.core_network_arn.clone(),
            // The answer reports a table's identity through `CoreNetworkSegmentEdge`, which
            // names a segment: a network function group is not one.
            segment_edge: Value::Null,
            timestamped: true,
            // No `create-route` action declares a route for a group, so the only routes here
            // are the ones the group's own peering propagates.
            routes: vec![route(
                INSPECTION_INSIDE_CIDR_BLOCK,
                ACTIVE,
                PROPAGATED,
                vec![peer_destination(
                    &subject.inspection_peer,
                    &subject.inspection_connect,
                    None,
                    Some(INSPECTION),
                )],
            )],
        },
        TableCase {
            name: "the network function group at the edge it holds no attachment at",
            identifier: group_edge(core_network, INSPECTION, CENTRAL),
            kind: NETWORK_FUNCTION_GROUP,
            arn: subject.core_network_arn.clone(),
            segment_edge: Value::Null,
            timestamped: true,
            routes: Vec::new(),
        },
        TableCase {
            name: "the route table of the peered transit gateway",
            identifier: json!({
                "TransitGatewayRouteTableArn": subject.transit_gateway_route_table_arn,
            }),
            kind: TRANSIT_GATEWAY_ROUTE_TABLE,
            arn: subject.transit_gateway_route_table_arn.clone(),
            segment_edge: Value::Null,
            timestamped: false,
            // The routes of a transit gateway's route table are that gateway's own, and this
            // service holds none of them.
            routes: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn every_route_table_of_a_global_network_is_reported_from_the_stored_policy_and_attachments()
{
    let emulator = Emulator::new();
    let subject = subject(&emulator).await;

    for case in table_cases(&subject) {
        let name = case.name;
        let body = read(
            &emulator,
            name,
            &subject.global_network,
            &case.identifier,
            &json!({}),
        )
        .await;

        assert_eq!(
            body["RouteTableType"],
            json!(case.kind),
            "case {name:?}: expected the route table type {:?}, got {body}",
            case.kind
        );
        assert_eq!(
            body["RouteTableArn"],
            json!(case.arn),
            "case {name:?}: expected the route table ARN {:?}, got {body}",
            case.arn
        );
        assert_eq!(
            body["CoreNetworkSegmentEdge"],
            case.segment_edge,
            "case {name:?}: expected the segment edge {}, got {body}",
            case.segment_edge
        );
        // The instant itself is one this run's clock fixed, so what is asserted is that the
        // table reports one exactly where this service knows it.
        assert_eq!(
            body["RouteTableTimestamp"].is_number(),
            case.timestamped,
            "case {name:?}: expected a route table timestamp: {}, got {body}",
            case.timestamped
        );
        assert_eq!(
            routes_of(name, &body),
            case.routes,
            "case {name:?}: the routes reported are not the ones the policy and the \
             attachments hold, got {body}"
        );
    }
}

/// One row of the match table: the members a read carries beside the identifier, and the
/// prefixes the answer must then report.
struct MatchCase {
    /// Names the case in every assertion message.
    name: &'static str,
    matches: Value,
    expected: Vec<&'static str>,
}

/// The table: one row per attribute a read can match on, over the route table of [`SHARED`]
/// at [`WEST`], whose unnarrowed answer [`west_shared`] states.
fn match_cases(subject: &Subject) -> Vec<MatchCase> {
    vec![
        MatchCase {
            name: "one exact CIDR match",
            matches: json!({ "ExactCidrMatches": ["10.2.0.0/16"] }),
            expected: vec!["10.2.0.0/16"],
        },
        MatchCase {
            name: "two exact CIDR matches, one per address family",
            matches: json!({
                "ExactCidrMatches": ["10.1.0.0/16", WEST_INSIDE_CIDR_BLOCKS[1]],
            }),
            expected: vec!["10.1.0.0/16", WEST_INSIDE_CIDR_BLOCKS[1]],
        },
        MatchCase {
            name: "an exact CIDR match spelled with host bits set",
            matches: json!({ "ExactCidrMatches": ["10.1.2.3/16"] }),
            expected: vec!["10.1.0.0/16"],
        },
        MatchCase {
            name: "an exact CIDR match no route holds",
            matches: json!({ "ExactCidrMatches": ["192.0.2.0/24"] }),
            expected: Vec::new(),
        },
        MatchCase {
            name: "the routes that are a subnet of a block",
            matches: json!({ "SubnetOfMatches": ["10.0.0.0/8"] }),
            expected: vec!["10.0.0.0/16", "10.0.0.0/8", "10.1.0.0/16", "10.2.0.0/16", "10.3.0.0/16"],
        },
        MatchCase {
            name: "the routes that are a subnet of either of two blocks",
            matches: json!({ "SubnetOfMatches": ["10.2.0.0/15", "192.168.0.0/16"] }),
            expected: vec!["10.2.0.0/16", "10.3.0.0/16", "192.168.0.0/24"],
        },
        MatchCase {
            name: "the routes that encompass a block",
            matches: json!({ "SupernetOfMatches": ["10.0.1.0/24"] }),
            expected: vec!["10.0.0.0/16", "10.0.0.0/8"],
        },
        MatchCase {
            name: "the most specific route matching one address",
            matches: json!({ "LongestPrefixMatches": ["10.0.1.1"] }),
            expected: vec!["10.0.0.0/16"],
        },
        MatchCase {
            name: "the most specific route matching a block the finer route does not hold",
            matches: json!({ "LongestPrefixMatches": ["10.9.0.0/16"] }),
            expected: vec!["10.0.0.0/8"],
        },
        MatchCase {
            name: "the most specific route matching either of two addresses",
            matches: json!({ "LongestPrefixMatches": ["10.3.0.4", "fd00::1"] }),
            expected: vec!["10.3.0.0/16", WEST_INSIDE_CIDR_BLOCKS[1]],
        },
        MatchCase {
            name: "the most specific route matching an address no route holds",
            matches: json!({ "LongestPrefixMatches": ["203.0.113.5"] }),
            expected: Vec::new(),
        },
        MatchCase {
            name: "the blackholed routes",
            matches: json!({ "States": [BLACKHOLE] }),
            expected: vec!["10.2.0.0/16", "192.168.0.0/24"],
        },
        MatchCase {
            name: "both route states, which narrows by nothing",
            matches: json!({ "States": [ACTIVE, BLACKHOLE] }),
            expected: vec![
                "10.0.0.0/16",
                "10.0.0.0/8",
                "10.1.0.0/16",
                "10.2.0.0/16",
                "10.3.0.0/16",
                WEST_INSIDE_CIDR_BLOCKS[0],
                "192.168.0.0/24",
                WEST_INSIDE_CIDR_BLOCKS[1],
            ],
        },
        MatchCase {
            name: "the propagated routes",
            matches: json!({ "Types": [PROPAGATED] }),
            expected: vec![WEST_INSIDE_CIDR_BLOCKS[1]],
        },
        MatchCase {
            name: "the routes reaching a VPC",
            matches: json!({ "DestinationFilters": { "RESOURCE_TYPE": ["vpc"] } }),
            expected: vec![
                "10.0.0.0/16",
                "10.0.0.0/8",
                "10.1.0.0/16",
                WEST_INSIDE_CIDR_BLOCKS[0],
            ],
        },
        MatchCase {
            name: "the routes reaching one named resource",
            matches: json!({
                "DestinationFilters": {
                    "RESOURCE_ID": [state::arn_resource_id(&subject.west_vpc.resource_arn)],
                },
            }),
            expected: vec![
                "10.0.0.0/16",
                "10.0.0.0/8",
                "10.1.0.0/16",
                WEST_INSIDE_CIDR_BLOCKS[0],
            ],
        },
        MatchCase {
            name: "the routes reaching a transit gateway attachment",
            matches: json!({
                "DestinationFilters": {
                    "TRANSIT_GATEWAY_ATTACHMENT_ID": [subject.transit_gateway_attachment_id],
                },
            }),
            expected: vec!["10.3.0.0/16"],
        },
        MatchCase {
            name: "two destination filters, which both have to select the route",
            matches: json!({
                "DestinationFilters": {
                    "RESOURCE_TYPE": ["vpc"],
                    "TRANSIT_GATEWAY_ATTACHMENT_ID": [subject.transit_gateway_attachment_id],
                },
            }),
            expected: Vec::new(),
        },
        MatchCase {
            name: "a prefix list, which no route of this service holds one of",
            matches: json!({ "PrefixListIds": [PREFIX_LIST_ID] }),
            expected: Vec::new(),
        },
        MatchCase {
            name: "a CIDR match and a type, which combine",
            matches: json!({
                "SubnetOfMatches": ["10.0.0.0/8"],
                "Types": [STATIC],
            }),
            expected: vec!["10.0.0.0/16", "10.0.0.0/8", "10.1.0.0/16", "10.2.0.0/16", "10.3.0.0/16"],
        },
        MatchCase {
            name: "a CIDR match and a state, which combine to nothing",
            matches: json!({
                "ExactCidrMatches": ["10.1.0.0/16"],
                "States": [BLACKHOLE],
            }),
            expected: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn every_match_narrows_the_answer_to_the_routes_whose_attribute_it_names() {
    let emulator = Emulator::new();
    let subject = subject(&emulator).await;
    let identifier = segment_edge(&subject.core_network, SHARED, WEST);

    for case in match_cases(&subject) {
        let name = case.name;
        let body = read(
            &emulator,
            name,
            &subject.global_network,
            &identifier,
            &case.matches,
        )
        .await;
        assert_eq!(
            prefixes_of(name, &body),
            case.expected,
            "case {name:?}: the narrowed answer is not the one the matches select, got {body}"
        );
        // Narrowing selects whole routes: every route reported still carries the destinations
        // and the type the unnarrowed table holds for its prefix.
        let held = west_shared(&subject);
        for route in routes_of(name, &body) {
            assert!(
                held.contains(&route),
                "case {name:?}: {route} is not one of the routes the table holds"
            );
        }
    }
}

/// One row of the refusal table: the global network and identifier a read is addressed by,
/// the matches it carries, and the refusal it must answer.
struct RefusalCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// Which global network the read is addressed at: the subject's, or the string itself.
    global_network: Option<&'static str>,
    identifier: Value,
    matches: Value,
    status: u16,
    code: &'static str,
    /// The `ValidationException$Fields` entry the refusal must name, or `None` where the
    /// refusal is a missing resource instead.
    field: Option<&'static str>,
}

/// The table: one row per way a request addresses no route table of the global network, or
/// carries a match this service cannot read.
fn refusal_cases(subject: &Subject, other_core_network: &str) -> Vec<RefusalCase> {
    let core_network = subject.core_network.as_str();
    vec![
        RefusalCase {
            name: "a global network nothing is stored under",
            global_network: Some("global-network-0123456789abcdef0"),
            identifier: segment_edge(core_network, SHARED, WEST),
            matches: json!({}),
            status: 404,
            code: "ResourceNotFoundException",
            field: None,
        },
        RefusalCase {
            name: "an identifier naming no route table at all",
            global_network: None,
            identifier: json!({}),
            matches: json!({}),
            status: 400,
            code: "ValidationException",
            field: Some("routeTableIdentifier"),
        },
        RefusalCase {
            name: "an identifier naming two route tables",
            global_network: None,
            identifier: json!({
                "TransitGatewayRouteTableArn": subject.transit_gateway_route_table_arn,
                "CoreNetworkSegmentEdge": {
                    "CoreNetworkId": core_network,
                    "SegmentName": SHARED,
                    "EdgeLocation": WEST,
                },
            }),
            matches: json!({}),
            status: 400,
            code: "ValidationException",
            field: Some("routeTableIdentifier"),
        },
        RefusalCase {
            name: "a segment edge naming no segment",
            global_network: None,
            identifier: json!({
                "CoreNetworkSegmentEdge": {
                    "CoreNetworkId": core_network,
                    "EdgeLocation": WEST,
                },
            }),
            matches: json!({}),
            status: 400,
            code: "ValidationException",
            field: Some("segmentName"),
        },
        RefusalCase {
            name: "a core network nothing is stored under",
            global_network: None,
            identifier: segment_edge("core-network-0123456789abcdef0", SHARED, WEST),
            matches: json!({}),
            status: 404,
            code: "ResourceNotFoundException",
            field: None,
        },
        RefusalCase {
            name: "a core network of another global network",
            global_network: None,
            identifier: segment_edge(other_core_network, SHARED, WEST),
            matches: json!({}),
            status: 404,
            code: "ResourceNotFoundException",
            field: None,
        },
        RefusalCase {
            name: "a segment the policy in force does not declare",
            global_network: None,
            identifier: segment_edge(core_network, "undeclared", WEST),
            matches: json!({}),
            status: 400,
            code: "ValidationException",
            field: Some("segmentName"),
        },
        RefusalCase {
            name: "a Region the policy in force configures no edge in",
            global_network: None,
            identifier: segment_edge(core_network, SHARED, "ap-southeast-2"),
            matches: json!({}),
            status: 400,
            code: "ValidationException",
            field: Some("edgeLocation"),
        },
        RefusalCase {
            name: "a network function group the policy in force does not declare",
            global_network: None,
            identifier: group_edge(core_network, "undeclared", WEST),
            matches: json!({}),
            status: 400,
            code: "ValidationException",
            field: Some("networkFunctionGroupName"),
        },
        RefusalCase {
            name: "a string that names no transit gateway route table",
            global_network: None,
            identifier: json!({ "TransitGatewayRouteTableArn": "tgw-rtb-0123abcd" }),
            matches: json!({}),
            status: 400,
            code: "ValidationException",
            field: Some("transitGatewayRouteTableArn"),
        },
        RefusalCase {
            name: "a transit gateway route table no attachment of this global network attaches",
            global_network: None,
            identifier: json!({
                "TransitGatewayRouteTableArn":
                    transit_gateway_route_table_arn(WEST, "00000000"),
            }),
            matches: json!({}),
            status: 404,
            code: "ResourceNotFoundException",
            field: None,
        },
        RefusalCase {
            name: "a destination filter naming no part of a destination",
            global_network: None,
            identifier: segment_edge(core_network, SHARED, WEST),
            matches: json!({ "DestinationFilters": { "SEGMENT_NAME": [SHARED] } }),
            status: 400,
            code: "ValidationException",
            field: Some("destinationFilters"),
        },
        RefusalCase {
            name: "a CIDR match that is not a CIDR block",
            global_network: None,
            identifier: segment_edge(core_network, SHARED, WEST),
            matches: json!({ "ExactCidrMatches": ["10.0.0.0-10.0.255.255"] }),
            status: 400,
            code: "ValidationException",
            field: Some("exactCidrMatches"),
        },
        RefusalCase {
            name: "a supernet match that is not a CIDR block",
            global_network: None,
            identifier: segment_edge(core_network, SHARED, WEST),
            matches: json!({ "SupernetOfMatches": ["everything"] }),
            status: 400,
            code: "ValidationException",
            field: Some("supernetOfMatches"),
        },
    ]
}

#[tokio::test]
async fn a_request_that_addresses_no_route_table_is_refused_on_the_member_that_carried_it() {
    let emulator = Emulator::new();
    let subject = subject(&emulator).await;

    // A core network of ANOTHER global network, so the row that names it addresses a core
    // network that exists but holds no route table this global network can be read for.
    let other_network = emulator.create_global_network("another global network").await;
    let (status, created) = create_core_network(
        &emulator,
        json!({
            "GlobalNetworkId": other_network,
            "Description": "another core network",
            "PolicyDocument": policy_document(&json!([])),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork for the neighbour expected 200, got {status} with {created}"
    );
    let other_core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no CoreNetworkId in {created}"))
        .to_string();

    for case in refusal_cases(&subject, &other_core_network) {
        let name = case.name;
        let global_network = case.global_network.unwrap_or(&subject.global_network);
        let (status, refused) =
            get_routes(&emulator, global_network, &case.identifier, &case.matches).await;
        assert_eq!(
            status, case.status,
            "case {name:?}: expected {}, got {status} with {refused}",
            case.status
        );
        assert_eq!(
            error_code(&refused),
            case.code,
            "case {name:?}: expected the error code {:?}, got {refused}",
            case.code
        );
        if let Some(field) = case.field {
            assert_eq!(
                refused["Fields"][0]["Name"],
                json!(field),
                "case {name:?}: the refusal must name the member at fault {field:?}, \
                 got {refused}"
            );
        }
    }
}

#[tokio::test]
async fn a_match_list_carrying_more_values_than_the_bound_allows_is_refused_on_its_member() {
    let emulator = Emulator::new();
    let subject = subject(&emulator).await;
    let identifier = segment_edge(&subject.core_network, SHARED, WEST);

    // One row per list a read matches on, each named by the member a refusal reports. The
    // bound is the one the service itself declares, read from the crate rather than mirrored
    // by hand.
    let over: Vec<String> = (0..=state::MAX_FILTER_IDS)
        .map(|index| format!("10.{index}.0.0/16"))
        .collect();
    let rows: Vec<(&str, Value)> = vec![
        (
            "exactCidrMatches",
            json!({ "ExactCidrMatches": over.clone() }),
        ),
        (
            "longestPrefixMatches",
            json!({ "LongestPrefixMatches": over.clone() }),
        ),
        (
            "subnetOfMatches",
            json!({ "SubnetOfMatches": over.clone() }),
        ),
        (
            "supernetOfMatches",
            json!({ "SupernetOfMatches": over.clone() }),
        ),
        ("prefixListIds", json!({ "PrefixListIds": over.clone() })),
        (
            "destinationFilters",
            json!({ "DestinationFilters": { "RESOURCE_ID": over.clone() } }),
        ),
        // The two enum lists: every value is one the model admits, so what the refusal is
        // about is the LENGTH of the list and nothing else.
        (
            "states",
            json!({ "States": vec![ACTIVE; state::MAX_FILTER_IDS + 1] }),
        ),
        (
            "types",
            json!({ "Types": vec![STATIC; state::MAX_FILTER_IDS + 1] }),
        ),
    ];

    for (member, matches) in rows {
        let (status, refused) =
            get_routes(&emulator, &subject.global_network, &identifier, &matches).await;
        assert_eq!(
            status, 400,
            "case {member:?}: a list of {} values is past the bound of {}, got {status} \
             with {refused}",
            over.len(),
            state::MAX_FILTER_IDS
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {member:?}: wrong error code in {refused}"
        );
        assert_eq!(
            refused["Fields"][0]["Name"],
            json!(member),
            "case {member:?}: the refusal must name the member that carried the list, \
             got {refused}"
        );
    }
}

#[tokio::test]
async fn reading_a_route_table_leaves_the_next_caller_the_same_answer() {
    let emulator = Emulator::new();
    let subject = subject(&emulator).await;
    let identifier = segment_edge(&subject.core_network, SHARED, WEST);

    // A read reports what the stored policy, attachments and peerings hold, so it writes
    // nothing: the second read answers exactly what the first did, and a narrowed read in
    // between changes nothing either.
    let first = read(
        &emulator,
        "the first read",
        &subject.global_network,
        &identifier,
        &json!({}),
    )
    .await;
    read(
        &emulator,
        "a narrowed read in between",
        &subject.global_network,
        &identifier,
        &json!({ "States": [BLACKHOLE] }),
    )
    .await;
    let second = read(
        &emulator,
        "the second read",
        &subject.global_network,
        &identifier,
        &json!({}),
    )
    .await;
    assert_eq!(
        second, first,
        "reading a route table must leave the next caller the same answer, got {second}"
    );
}
