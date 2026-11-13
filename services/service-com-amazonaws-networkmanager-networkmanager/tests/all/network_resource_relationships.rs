// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetNetworkResourceRelationships`, driven over HTTP.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 route, the
//! `resourceType` / `resourceArn` / `accountId` / `awsRegion` / `coreNetworkId` /
//! `registeredGatewayArn` / `maxResults` / `nextToken` query bindings and the error
//! envelopes are exercised as a client hits them. Nothing reaches into `crate::state`:
//! every pair expected below is built from the ties the test made through the operations
//! that make them (`CreateLink` / `CreateDevice` / `AssociateLink` / `CreateConnection` /
//! `CreateVpcAttachment` / `AssociateCustomerGateway` /
//! `CreateCoreNetworkPrefixListAssociation`).
//!
//! The rule under test: for the global network the path addresses, the answer carries one
//! `Relationship` per tie the store holds between two resources named by ARN — the parent
//! that owns a resource of that global network's network-resource inventory, the resource
//! an attachment or a peering joins to it, and each resource an association ties it to —
//! with `From` the resource the tie is recorded on and `To` the resource at the other end,
//! narrowed to the relationships one of whose two resources every filter member the
//! request carries selects, paginated by `MaxResults`/`NextToken`, and reading never
//! changes the graph.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, create_vpc_attachment, policy_document, subnet_arn, vpc_arn,
};
use crate::connection_lifecycle::{create_connection_ok, id_of};
use crate::core_network_lifecycle::create_core_network;
use crate::customer_gateway_association_lifecycle::register_vpn_carrier;
use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, expected_arn, expected_scoped_arn, Emulator};
use crate::link_association_lifecycle::associate_ok;
use crate::link_lifecycle::{create_link_ok, plain_link};
use crate::network_resource_counts::SUPPORTED_TYPES;
use crate::network_resources::minted;
use crate::resource_tagging::escape;

/// The Region the fixture's core network configures its one edge location in, which is
/// therefore the Region its VPC attachment and that attachment's VPC sit in.
const EDGE_REGION: &str = "us-west-2";

/// The Region of the customer gateway the fixture associates with a device, and the one of
/// the managed prefix list it associates with the core network: both resources of Amazon
/// VPC, each in a Region nothing else of the fixture sits in, so an `awsRegion` filter
/// naming either selects exactly the ties that reach it.
const GATEWAY_REGION: &str = "eu-central-1";
/// See [`GATEWAY_REGION`].
const PREFIX_LIST_REGION: &str = "eu-west-1";

/// The account those two Amazon VPC resources belong to, which is NOT the account this
/// emulator holds its own resources in.
const FOREIGN_ACCOUNT: &str = "123456789012";

/// The `GetNetworkResourceRelationships` URI of `network`, with `query` (already escaped)
/// appended when it is not empty.
fn relationships_uri(network: &str, query: &str) -> String {
    let path = format!("/global-networks/{network}/network-resource-relationships");
    if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    }
}

/// One relationship as the answer carries it: the ARN it leads FROM and the ARN it names.
type Pair = (String, String);

/// One row of the filter table below: the case, the query string, and the pairs that query
/// must select.
type FilterRow = (String, String, Vec<Pair>);

/// The pairs one answer carries, in the order returned.
fn pairs_of(body: &Value) -> Vec<Pair> {
    body["Relationships"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a Relationships array in {body}"))
        .iter()
        .map(|relationship| {
            let from = relationship["From"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a From in {relationship}"));
            let to = relationship["To"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a To in {relationship}"));
            (from.to_string(), to.to_string())
        })
        .collect()
}

/// The key a pair is listed under: the ARN it leads from, then the one it names.
fn key(pair: &Pair) -> String {
    format!("{}|{}", pair.0, pair.1)
}

/// `pairs` in the order the answer lists them — by the resource each leads from, then by
/// the one it reaches — holding a pair two ties both name exactly once.
fn ordered(mut pairs: Vec<Pair>) -> Vec<Pair> {
    pairs.sort_by_key(key);
    pairs.dedup();
    pairs
}

/// Every pair `query` selects, following `NextToken` to the end, with the page count.
async fn listed(emulator: &Emulator, network: &str, query: &str) -> (Vec<Pair>, usize) {
    let mut collected: Vec<Pair> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut full = query.to_string();
        if let Some(next) = &token {
            if !full.is_empty() {
                full.push('&');
            }
            full.push_str(&format!("nextToken={}", escape(next)));
        }
        let (status, body) = emulator.get(&relationships_uri(network, &full)).await;
        assert_eq!(
            status, 200,
            "case {query:?} page {pages}: expected 200, got {status} with {body}"
        );
        collected.extend(pairs_of(&body));
        pages += 1;
        assert!(
            pages <= 64,
            "case {query:?}: pagination did not terminate after {pages} pages"
        );
        token = body["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    (collected, pages)
}

/// The ARNs of the resources the fixture below ties together, plus the ids a filter names
/// them by.
struct Graph {
    network: String,
    core_network_id: String,
    site_a: String,
    site_b: String,
    link_a: String,
    link_b: String,
    device_a: String,
    device_b: String,
    connection: String,
    core_network: String,
    attachment: String,
    vpc: String,
    gateway: String,
    prefix_list: String,
}

impl Graph {
    /// Every pair the fixture's ties make: a link and a device at their site, the two
    /// devices and two links a connection joins, an attachment's core network and the VPC
    /// it attaches, the link a link association ties to a device, the customer gateway an
    /// association ties to that device and that link, and the prefix list associated with
    /// the core network.
    fn expected(&self) -> Vec<Pair> {
        ordered(vec![
            (self.link_a.clone(), self.site_a.clone()),
            (self.link_b.clone(), self.site_b.clone()),
            (self.device_a.clone(), self.site_a.clone()),
            (self.connection.clone(), self.device_a.clone()),
            (self.connection.clone(), self.device_b.clone()),
            (self.connection.clone(), self.link_a.clone()),
            (self.connection.clone(), self.link_b.clone()),
            (self.attachment.clone(), self.core_network.clone()),
            (self.attachment.clone(), self.vpc.clone()),
            (self.device_a.clone(), self.link_a.clone()),
            (self.device_a.clone(), self.gateway.clone()),
            (self.link_a.clone(), self.gateway.clone()),
            (self.core_network.clone(), self.prefix_list.clone()),
        ])
    }

    /// The pairs of [`Self::expected`] that reach `arn` from either end — what a filter
    /// selecting exactly that one resource must answer.
    fn touching(&self, arn: &str) -> Vec<Pair> {
        self.expected()
            .into_iter()
            .filter(|(from, to)| from == arn || to == arn)
            .collect()
    }

    /// The pairs of [`Self::expected`] that reach any ARN of `arns`.
    fn touching_any(&self, arns: &[&String]) -> Vec<Pair> {
        self.expected()
            .into_iter()
            .filter(|(from, to)| arns.iter().any(|arn| from == *arn || to == *arn))
            .collect()
    }
}

/// The graph the tests below run against: ONE global network whose ties span every kind
/// this service records — a child at its parent (a link and a device at their site), a
/// resource of its own joining two others (a connection over two devices and two links),
/// an attachment at its core network and at the VPC it attaches, and three associations
/// that are no resource of their own (a device to a link, a customer gateway to that
/// device and link, a managed prefix list to the core network).
///
/// The rows differ along what the reporting rule generalises over: the kind of tie, which
/// end of it the store records, whether the other end is a resource of this service or of
/// another one, and whether a resource is tied to anything at all — the second device is
/// at no site, so it is reached only through the connection.
async fn graph(emulator: &Emulator) -> Graph {
    let network = emulator.create_global_network("resource relationships").await;
    let site_a = emulator.create_site(&network, "site a").await;
    let site_b = emulator.create_site(&network, "site b").await;
    let link_a = create_link_ok(emulator, &network, &site_a, &plain_link()).await;
    let link_b = create_link_ok(emulator, &network, &site_b, &plain_link()).await;
    let device_a = create_device_ok(emulator, &network, json!({ "SiteId": site_a })).await;
    let device_b = create_device_ok(emulator, &network, json!({})).await;
    associate_ok(emulator, &network, &device_a, &link_a).await;

    let connection = id_of(
        &create_connection_ok(
            emulator,
            &network,
            json!({
                "DeviceId": device_a,
                "ConnectedDeviceId": device_b,
                "LinkId": link_a,
                "ConnectedLinkId": link_b,
            }),
        )
        .await,
    );

    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": network,
            "PolicyDocument": policy_document(&[EDGE_REGION]),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetwork: expected 200, got {status} with {created}"
    );
    let core_network_id = minted(&created, "CoreNetwork", "CoreNetworkId");

    let vpc = vpc_arn(EDGE_REGION, 1);
    let (status, created) = create_vpc_attachment(
        emulator,
        json!({
            "CoreNetworkId": core_network_id,
            "VpcArn": vpc,
            "SubnetArns": [subnet_arn(EDGE_REGION, 2)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment: expected 200, got {status} with {created}"
    );
    let attachment = attachment_id("vpc attachment", &created["VpcAttachment"]);

    // A customer gateway of Amazon VPC, tied to the device AND to the link that device
    // reaches it over ("If you specify a link, it must be associated with the specified
    // device", which the link association above is). The tie also stands on a carrier in the
    // gateway's own Region — a transit gateway or core network edge its VPN attachment could
    // be on — and a registration is no network resource of this inventory, so registering one
    // leaves every listing below as it was.
    let gateway =
        format!("arn:aws:ec2:{GATEWAY_REGION}:{FOREIGN_ACCOUNT}:customer-gateway/cgw-0123abcd");
    register_vpn_carrier(emulator, &network, GATEWAY_REGION).await;
    let (status, body) = emulator
        .post(
            &format!("/global-networks/{network}/customer-gateway-associations"),
            json!({
                "CustomerGatewayArn": gateway,
                "DeviceId": device_a,
                "LinkId": link_a,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "AssociateCustomerGateway: expected 200, got {status} with {body}"
    );

    let prefix_list =
        format!("arn:aws:ec2:{PREFIX_LIST_REGION}:{FOREIGN_ACCOUNT}:prefix-list/pl-0123abcd");
    let (status, body) = emulator
        .post(
            "/prefix-list",
            json!({
                "CoreNetworkId": core_network_id,
                "PrefixListArn": prefix_list,
                "PrefixListAlias": "shared",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateCoreNetworkPrefixListAssociation: expected 200, got {status} with {body}"
    );

    Graph {
        site_a: expected_scoped_arn("site", &network, &site_a),
        site_b: expected_scoped_arn("site", &network, &site_b),
        link_a: expected_scoped_arn("link", &network, &link_a),
        link_b: expected_scoped_arn("link", &network, &link_b),
        device_a: expected_scoped_arn("device", &network, &device_a),
        device_b: expected_scoped_arn("device", &network, &device_b),
        connection: expected_scoped_arn("connection", &network, &connection),
        core_network: expected_arn("core-network", &core_network_id),
        attachment: expected_arn("attachment", &attachment),
        vpc,
        gateway,
        prefix_list,
        core_network_id,
        network,
    }
}

#[tokio::test]
async fn every_stored_tie_is_reported_once_as_the_pair_of_arns_it_joins() {
    let emulator = Emulator::new();
    let graph = graph(&emulator).await;

    let (reported, _) = listed(&emulator, &graph.network, "").await;
    assert_eq!(
        reported,
        graph.expected(),
        "expected every stored tie once, ordered by the resource it leads from, got {reported:?}"
    );

    // A global network holding no tie at all reports an empty graph rather than another
    // network's: the listing is per global network.
    let bare = emulator.create_global_network("no ties").await;
    let (empty, pages) = listed(&emulator, &bare, "").await;
    assert_eq!(
        empty,
        Vec::<Pair>::new(),
        "case a global network with no ties: expected no relationships, got {empty:?}"
    );
    assert_eq!(
        pages, 1,
        "case a global network with no ties: expected one page, got {pages}"
    );
}

#[tokio::test]
async fn each_filter_narrows_the_graph_to_the_ties_reaching_a_resource_it_selects() {
    let emulator = Emulator::new();
    let graph = graph(&emulator).await;
    let all = graph.expected();

    // A resource of ANOTHER global network: the graph is per global network, so its ARN
    // selects nothing here.
    let other_network = emulator.create_global_network("elsewhere").await;
    let other_site = emulator.create_site(&other_network, "elsewhere").await;
    let other_site_arn = expected_scoped_arn("site", &other_network, &other_site);
    let none: Vec<Pair> = Vec::new();

    let mut rows: Vec<FilterRow> = vec![
        ("no filter at all".to_string(), String::new(), all.clone()),
        (
            "resourceArn of a site".to_string(),
            format!("resourceArn={}", escape(&graph.site_a)),
            graph.touching(&graph.site_a),
        ),
        (
            "resourceArn of a link at both ends of ties".to_string(),
            format!("resourceArn={}", escape(&graph.link_a)),
            graph.touching(&graph.link_a),
        ),
        (
            "resourceArn of the connection".to_string(),
            format!("resourceArn={}", escape(&graph.connection)),
            graph.touching(&graph.connection),
        ),
        (
            "resourceArn of a device tied to nothing but the connection".to_string(),
            format!("resourceArn={}", escape(&graph.device_b)),
            graph.touching(&graph.device_b),
        ),
        (
            "resourceArn of the customer gateway of another service".to_string(),
            format!("resourceArn={}", escape(&graph.gateway)),
            graph.touching(&graph.gateway),
        ),
        (
            "resourceArn of the attached VPC".to_string(),
            format!("resourceArn={}", escape(&graph.vpc)),
            graph.touching(&graph.vpc),
        ),
        (
            "resourceArn of the associated prefix list".to_string(),
            format!("resourceArn={}", escape(&graph.prefix_list)),
            graph.touching(&graph.prefix_list),
        ),
        (
            "resourceArn of another global network's site".to_string(),
            format!("resourceArn={}", escape(&other_site_arn)),
            none.clone(),
        ),
        (
            "resourceArn naming nothing stored".to_string(),
            format!(
                "resourceArn={}",
                escape(&expected_scoped_arn(
                    "site",
                    "global-network-000000000000000ff",
                    "site-000000000000000ff"
                ))
            ),
            none.clone(),
        ),
        (
            "accountId of the account holding this service's resources".to_string(),
            format!("accountId={}", engine::emulator_account_id()),
            all.clone(),
        ),
        (
            "accountId of the account holding the Amazon VPC resources".to_string(),
            format!("accountId={FOREIGN_ACCOUNT}"),
            graph.touching_any(&[&graph.gateway, &graph.prefix_list]),
        ),
        (
            "accountId of an account holding none of them".to_string(),
            "accountId=111122223333".to_string(),
            none.clone(),
        ),
        (
            "coreNetworkId of the stored core network".to_string(),
            format!("coreNetworkId={}", graph.core_network_id),
            graph.touching_any(&[&graph.core_network, &graph.attachment]),
        ),
        (
            "coreNetworkId of a core network this global network does not hold".to_string(),
            "coreNetworkId=core-network-000000000000000ff".to_string(),
            none.clone(),
        ),
        (
            "registeredGatewayArn of a gateway".to_string(),
            format!(
                "registeredGatewayArn={}",
                escape("arn:aws:ec2:us-west-2:000000000000:transit-gateway/tgw-0123456789abcdef0")
            ),
            none.clone(),
        ),
        (
            "two filters, both satisfied by one resource".to_string(),
            format!("resourceType=link&resourceArn={}", escape(&graph.link_a)),
            graph.touching(&graph.link_a),
        ),
        (
            "two filters, satisfied by no single resource".to_string(),
            format!("resourceType=site&resourceArn={}", escape(&graph.link_a)),
            none.clone(),
        ),
    ];

    // Every documented resource type is a row: the kinds this global network holds select
    // the ties that reach one of them, and every other documented kind is tied to nothing
    // here, so it selects nothing rather than being ignored.
    let of_type: Vec<(&str, Vec<&String>)> = vec![
        ("site", vec![&graph.site_a, &graph.site_b]),
        ("link", vec![&graph.link_a, &graph.link_b]),
        ("device", vec![&graph.device_a, &graph.device_b]),
        ("connection", vec![&graph.connection]),
        ("core-network", vec![&graph.core_network]),
        ("attachment", vec![&graph.attachment]),
        ("customer-gateway", vec![&graph.gateway]),
    ];
    for resource_type in SUPPORTED_TYPES {
        let arns = of_type
            .iter()
            .find(|(kind, _)| *kind == resource_type)
            .map_or_else(Vec::new, |(_, arns)| arns.clone());
        rows.push((
            format!("resourceType={resource_type}"),
            format!("resourceType={}", escape(resource_type)),
            graph.touching_any(&arns),
        ));
    }

    // Every Region is a row: the edge Region selects the attachment and the VPC it
    // attaches, each foreign resource's Region selects the ties that reach it, and a Region
    // nothing of this global network sits in selects nothing.
    let of_region: Vec<(&str, Vec<&String>)> = vec![
        (EDGE_REGION, vec![&graph.attachment, &graph.vpc]),
        (GATEWAY_REGION, vec![&graph.gateway]),
        (PREFIX_LIST_REGION, vec![&graph.prefix_list]),
        ("ap-southeast-1", Vec::new()),
    ];
    for (region, arns) in of_region {
        rows.push((
            format!("awsRegion={region}"),
            format!("awsRegion={region}"),
            graph.touching_any(&arns),
        ));
    }

    for (case, query, expected) in rows {
        let (selected, _) = listed(&emulator, &graph.network, &query).await;
        assert_eq!(
            selected, expected,
            "case {case} ({query:?}): expected {expected:?}, got {selected:?}"
        );
    }
}

#[tokio::test]
async fn pagination_lists_every_relationship_exactly_once_whatever_the_page_size() {
    let emulator = Emulator::new();
    let graph = graph(&emulator).await;
    let expected = graph.expected();
    let held = expected.len();

    // `MaxResults` is modelled `range 1..500`, so the rows are no page size at all, each
    // published bound, and the sizes either side of the graph's own size.
    for page_size in [
        None,
        Some(1),
        Some(2),
        Some(held - 1),
        Some(held),
        Some(held + 1),
        Some(500),
    ] {
        let query = page_size.map_or_else(String::new, |size| format!("maxResults={size}"));
        let (reported, pages) = listed(&emulator, &graph.network, &query).await;
        assert_eq!(
            reported, expected,
            "case maxResults={page_size:?}: expected every relationship once in order, got {reported:?}"
        );
        let expected_pages = page_size.map_or(1, |size| held.div_ceil(size));
        assert_eq!(
            pages, expected_pages,
            "case maxResults={page_size:?}: expected {expected_pages} pages over {held} relationships, got {pages}"
        );
    }

    // A page while items remain is never empty, and never longer than the size asked for.
    let (status, first) = emulator
        .get(&relationships_uri(&graph.network, "maxResults=1"))
        .await;
    assert_eq!(
        status, 200,
        "first page: expected 200, got {status} with {first}"
    );
    assert_eq!(
        pairs_of(&first).len(),
        1,
        "first page: expected exactly the one relationship asked for, got {first}"
    );
    assert!(
        first["NextToken"].as_str().is_some(),
        "first page: relationships remain, so a token must resume after the page, got {first}"
    );
}

#[tokio::test]
async fn a_tie_is_in_the_graph_only_while_the_store_holds_it() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("ties come and go").await;
    let site = emulator.create_site(&network, "one site").await;
    let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
    let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;

    let site_arn = expected_scoped_arn("site", &network, &site);
    let link_arn = expected_scoped_arn("link", &network, &link);
    let device_arn = expected_scoped_arn("device", &network, &device);
    let parents = ordered(vec![
        (link_arn.clone(), site_arn.clone()),
        (device_arn.clone(), site_arn.clone()),
    ]);
    let association = (device_arn.clone(), link_arn.clone());

    let (before, _) = listed(&emulator, &network, "").await;
    assert_eq!(
        before, parents,
        "before the association: expected only the ties to the site, got {before:?}"
    );

    associate_ok(&emulator, &network, &device, &link).await;
    let mut with_association = parents.clone();
    with_association.push(association.clone());
    let with_association = ordered(with_association);
    let (made, _) = listed(&emulator, &network, "").await;
    assert_eq!(
        made, with_association,
        "after AssociateLink: expected the device/link pair as well, got {made:?}"
    );

    // Reading the graph is a read: a later caller sees the same answer.
    for repeat in 1..=3 {
        let (again, _) = listed(&emulator, &network, "").await;
        assert_eq!(
            again, with_association,
            "case repeat {repeat}: reading changed the graph, got {again:?}"
        );
    }

    let (status, body) = emulator
        .delete(&format!(
            "/global-networks/{network}/link-associations?deviceId={device}&linkId={link}"
        ))
        .await;
    assert_eq!(
        status, 200,
        "DisassociateLink: expected 200, got {status} with {body}"
    );
    let (after, _) = listed(&emulator, &network, "").await;
    assert_eq!(
        after, parents,
        "after DisassociateLink: expected the device/link pair gone, got {after:?}"
    );
}

#[tokio::test]
async fn a_resource_type_outside_the_documented_set_is_refused_with_the_modelled_validation_error()
{
    let emulator = Emulator::new();
    let network = emulator.create_global_network("refusals").await;

    // Each row is a different WAY of being outside the documented set: an invented name, a
    // documented name in the wrong case, pluralised, padded, misspelled with the wrong
    // separator, a prefix of a documented name, and identifiers that are not types.
    for (case, resource_type) in [
        ("an invented type", "not-a-real-type"),
        ("a documented type in the wrong case", "Device"),
        ("a documented type pluralised", "devices"),
        ("a documented type with a trailing space", "device "),
        ("an underscore instead of a hyphen", "core_network"),
        ("a prefix of a documented type", "transit"),
        ("a resource id instead of a type", "site-00000000000000001"),
        (
            "an ARN instead of a type",
            "arn:aws:networkmanager::000000000000:site/site-00000000000000001",
        ),
    ] {
        let query = format!("resourceType={}", escape(resource_type));
        let (status, body) = emulator.get(&relationships_uri(&network, &query)).await;
        assert_eq!(
            status, 400,
            "case {case} ({resource_type:?}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} ({resource_type:?}): wrong error code in {body}"
        );
    }
}

#[tokio::test]
async fn a_global_network_the_store_does_not_hold_is_not_found_and_a_foreign_token_is_refused() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("refusals").await;
    let deleted = emulator.create_global_network("to be deleted").await;
    let (status, body) = emulator.delete(&format!("/global-networks/{deleted}")).await;
    assert_eq!(
        status, 200,
        "DeleteGlobalNetwork: expected 200, got {status} with {body}"
    );

    for (case, missing) in [
        ("never created", "global-network-00000000000000099".to_string()),
        ("deleted", deleted),
        ("not an id of this service", "nope".to_string()),
    ] {
        let (status, body) = emulator.get(&relationships_uri(&missing, "")).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: wrong error code in {body}"
        );
        assert_eq!(
            body["ResourceType"],
            json!("global-network"),
            "case {case}: the refusal must name the resource type, got {body}"
        );
        assert_eq!(
            body["ResourceId"],
            json!(missing),
            "case {case}: the refusal must name the addressed id, got {body}"
        );
    }

    // A NON-empty token this service did not mint names no offset into the listing, so it
    // is refused on the member rather than silently answered from the first page. The
    // empty string is no token at all — it satisfies the modelled `length 0..2048` and
    // names no position, so it asks for the first page (`crate::empty_page_token`).
    for (case, token) in [
        ("not hex at all", "not-a-token"),
        ("hex of foreign text", "6e6f742d612d746f6b656e"),
        ("a truncated token", "6e6574776f726b6d616e61"),
    ] {
        let query = format!("nextToken={}", escape(token));
        let (status, body) = emulator.get(&relationships_uri(&network, &query)).await;
        assert_eq!(
            status, 400,
            "case {case} ({token:?}): expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case} ({token:?}): wrong error code in {body}"
        );
    }
}
