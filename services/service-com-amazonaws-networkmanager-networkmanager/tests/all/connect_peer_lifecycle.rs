// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the core network Connect peer slice — `CreateConnectPeer`,
//! `GetConnectPeer` and `ListConnectPeers` — driven over HTTP through the real router (the
//! harness lives in `crate::global_network_lifecycle`).
//!
//! The rule under test: a Connect peer is a stored resource of ONE Connect attachment, so
//! it takes that attachment's core network, edge location and tunnel protocol rather than
//! any of its own, keeps the two tunnel addresses, the inside CIDR blocks, the peer ASN,
//! the subnet and the tags its create carried, peers over the ASN its core network policy
//! assigns that edge, and waits in `CREATING` until the transition the clock owes falls due.
//! `ListConnectPeers` then reports one summary per stored peer its filters select — an id
//! naming nothing selects none, since the operation models no not-found failure — paged by
//! `MaxResults`/`NextToken` in the stable peer-id order, and reading changes nothing a later
//! caller sees.
//!
//! The table below spans that class: rows differing in tunnel protocol (GRE, NO_ENCAP), in
//! edge location, in address family (IPv4, IPv6), in how many inside CIDR blocks are named
//! (none, one, two), in peer ASN (absent, and each published bound of the AS number domain),
//! in whether a subnet is named, and in how many tags are carried. Nothing reaches into
//! `crate::state`: every peer is created and read back through an operation.

use serde_json::{json, Value};

use crate::attachment_lifecycle::{
    attachment_id, create_connect_attachment, create_vpc_attachment, subnet_arn, vpc_arn,
};
use crate::core_network_lifecycle::create_core_network;
use crate::global_network_lifecycle::{
    error_code, expected_arn, is_minted_id, Emulator, PAST_SETTLE,
};
use crate::resource_tagging::{list_tags, tag};

/// The id prefix and ARN resource type of a Connect peer: the model's own
/// `^connect-peer-([0-9a-f]{8,17})$` (`smithy_query shape=ConnectPeerId`).
///
/// `pub(crate)` because `tests/all/connect_peer_delete.rs` drives the same resource, so one
/// definition of the peer table and its harness serves both suites.
pub(crate) const CONNECT_PEER: &str = "connect-peer";

/// The largest page a caller may ask for: `MaxResults` is modelled `range 1..500`
/// (`smithy_query query="operation" operation="ListConnectPeers"`).
const MAX_PAGE: usize = 500;

/// How many inside CIDR blocks one peer may name: the bound
/// `state::MAX_INSIDE_CIDR_BLOCKS_PER_CONNECT_PEER` fixes, which is what keeps one peer's
/// list from growing without end.
const MAX_INSIDE_CIDR_BLOCKS: usize = 10;

/// The AS number domain a BGP ASN lives in: a 32-bit number with AS 0 reserved, so the
/// assignable numbers are 1..=4294967295. `BgpOptions$PeerAsn` is a modelled `Long` with no
/// range of its own (`smithy_query shape=BgpOptions`), so both bounds are exercised below.
const ASN_MIN: i64 = 1;
/// See [`ASN_MIN`].
const ASN_MAX: i64 = 4_294_967_295;

/// The edges the core network under test configures: a Region, and the ASN its policy
/// assigns the edge there — the core network's own end of every BGP session that edge peers
/// over.
const EDGES: [(&str, i64); 3] = [
    ("us-west-2", 64512),
    ("eu-central-1", 64513),
    ("ap-southeast-4", 64514),
];

/// A core network policy document configuring one edge per entry of `edges`, each carrying
/// that entry's ASN.
///
/// The key names (`core-network-configuration`, `asn-ranges`, `edge-locations`, `location`,
/// `asn`, `segments`, `name`) are the published Cloud WAN core network policy schema's, the
/// same source `crate::attachment_lifecycle::policy_document` builds its documents from;
/// this one names each edge's `asn` explicitly so the ASN a peer reports is the document's
/// and not a number the test guessed.
fn policy_document(edges: &[(&str, i64)]) -> String {
    let locations: Vec<Value> = edges
        .iter()
        .map(|(location, asn)| json!({ "location": location, "asn": asn }))
        .collect();
    json!({
        "version": "2021.12",
        "core-network-configuration": {
            "asn-ranges": ["64512-64555"],
            "edge-locations": locations,
        },
        "segments": [{ "name": "shared" }],
    })
    .to_string()
}

/// One Connect attachment of the subject: the Region it sits at and the tunnel protocol its
/// options ask for (`TunnelProtocol` is GRE or NO_ENCAP, `smithy_query
/// shape=TunnelProtocol`).
pub(crate) struct Transport {
    pub(crate) region: &'static str,
    protocol: &'static str,
}

/// The Connect attachments every row below hangs a peer off: both protocols, and three edge
/// locations between them.
pub(crate) const TRANSPORTS: [Transport; 4] = [
    Transport {
        region: "us-west-2",
        protocol: "GRE",
    },
    Transport {
        region: "eu-central-1",
        protocol: "GRE",
    },
    Transport {
        region: "ap-southeast-4",
        protocol: "NO_ENCAP",
    },
    Transport {
        region: "us-west-2",
        protocol: "NO_ENCAP",
    },
];

/// The ASN the policy assigns the edge in `region`.
fn edge_asn(region: &str) -> i64 {
    EDGES
        .iter()
        .find(|(location, _)| *location == region)
        .map(|(_, asn)| *asn)
        .unwrap_or_else(|| panic!("the subject configures no edge in {region}"))
}

/// A built subject: a global network, its core network, and one Connect attachment per
/// entry of [`TRANSPORTS`], in that order.
pub(crate) struct Subject {
    pub(crate) global_network: String,
    pub(crate) core_network: String,
    pub(crate) attachments: Vec<String>,
}

/// Build a subject. `base` offsets the VPC and subnet ARNs its transports attach, so two
/// subjects of one emulator attach different VPCs.
pub(crate) async fn subject(emulator: &Emulator, name: &str, base: usize) -> Subject {
    let global_network = emulator.create_global_network(name).await;
    let (status, created) = create_core_network(
        emulator,
        json!({
            "GlobalNetworkId": global_network,
            "Description": name,
            "PolicyDocument": policy_document(&EDGES),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateCoreNetwork expected 200, got {status} with {created}"
    );
    let core_network = created["CoreNetwork"]["CoreNetworkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no CoreNetworkId in {created}"))
        .to_string();

    let mut attachments = Vec::new();
    for (index, transport) in TRANSPORTS.iter().enumerate() {
        let position = base + index;
        // A Connect attachment is carried over a VPC attachment of the same core network,
        // so each gets its own transport: a VPC is attached by at most one attachment.
        let (status, created) = create_vpc_attachment(
            emulator,
            json!({
                "CoreNetworkId": core_network,
                "VpcArn": vpc_arn(transport.region, position),
                "SubnetArns": [subnet_arn(transport.region, position)],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateVpcAttachment expected 200, got {status} with {created}"
        );
        let vpc = attachment_id(name, &created["VpcAttachment"]);

        let (status, created) = create_connect_attachment(
            emulator,
            json!({
                "CoreNetworkId": core_network,
                "EdgeLocation": transport.region,
                "TransportAttachmentId": vpc,
                "Options": { "Protocol": transport.protocol },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateConnectAttachment expected 200, got {status} with {created}"
        );
        attachments.push(attachment_id(name, &created["ConnectAttachment"]));
    }

    Subject {
        global_network,
        core_network,
        attachments,
    }
}

/// One row of the table: the peer one `CreateConnectPeer` asks for.
pub(crate) struct PeerCase {
    /// Names the case in every assertion message.
    pub(crate) name: &'static str,
    /// Which entry of [`TRANSPORTS`] the peer is carried over.
    pub(crate) transport: usize,
    /// The appliance's address, which every peering requires.
    peer_address: &'static str,
    /// The core network's address, which only a GRE peering needs.
    core_network_address: Option<&'static str>,
    /// The inside IP addresses the BGP session runs over.
    inside_cidr_blocks: &'static [&'static str],
    /// The `BgpOptions$PeerAsn` the request names, or `None` for a request naming none.
    peer_asn: Option<i64>,
    /// Whether the request names a subnet (one in the transport's own Region).
    pub(crate) subnet: bool,
    /// The tags the create carries.
    pub(crate) tags: &'static [(&'static str, &'static str)],
}

/// The table: rows differing along everything the rule generalises over — the protocol and
/// edge of the attachment the peer hangs off, the address family of its two ends, how many
/// inside CIDR blocks it peers over, its peer ASN, whether it names a subnet, and its tags.
pub(crate) const PEER_CASES: [PeerCase; 6] = [
    PeerCase {
        name: "a GRE peer with both tunnel addresses, one inside CIDR block and a peer ASN",
        transport: 0,
        peer_address: "203.0.113.1",
        core_network_address: Some("203.0.113.2"),
        inside_cidr_blocks: &["169.254.10.0/29"],
        peer_asn: Some(65000),
        subnet: false,
        tags: &[],
    },
    PeerCase {
        name: "a GRE peer naming no core network address, no inside CIDR blocks and no ASN",
        transport: 1,
        peer_address: "198.51.100.7",
        core_network_address: None,
        inside_cidr_blocks: &[],
        peer_asn: None,
        subnet: false,
        tags: &[("Name", "edge")],
    },
    PeerCase {
        name: "a GRE peer peering over an IPv4 and an IPv6 inside CIDR block, at the top of the ASN domain",
        transport: 0,
        peer_address: "203.0.113.9",
        core_network_address: Some("203.0.113.10"),
        inside_cidr_blocks: &["169.254.11.0/29", "fd00::/125"],
        peer_asn: Some(ASN_MAX),
        subnet: false,
        tags: &[("Name", "dual-stack"), ("Tier", "edge")],
    },
    PeerCase {
        name: "a GRE peer whose two ends are IPv6 addresses",
        transport: 1,
        peer_address: "2001:db8::1",
        core_network_address: Some("2001:db8::2"),
        inside_cidr_blocks: &["fd00:1::/125"],
        peer_asn: Some(ASN_MIN),
        subnet: false,
        tags: &[],
    },
    PeerCase {
        name: "a NO_ENCAP peer reaching its appliance through a subnet",
        transport: 2,
        peer_address: "192.0.2.20",
        core_network_address: None,
        inside_cidr_blocks: &["169.254.12.0/29"],
        peer_asn: Some(64600),
        subnet: true,
        tags: &[("Name", "appliance")],
    },
    PeerCase {
        name: "a NO_ENCAP peer naming no subnet at all",
        transport: 3,
        peer_address: "192.0.2.30",
        core_network_address: None,
        inside_cidr_blocks: &[],
        peer_asn: None,
        subnet: false,
        tags: &[],
    },
];

/// The subnet ARN a row that names one carries: one in the Region its transport sits at.
pub(crate) fn case_subnet(case: &PeerCase) -> String {
    subnet_arn(TRANSPORTS[case.transport].region, 900 + case.transport)
}

/// The `CreateConnectPeer` request body a row asks for.
pub(crate) fn request_of(case: &PeerCase, attachment: &str) -> Value {
    let mut body = json!({
        "ConnectAttachmentId": attachment,
        "PeerAddress": case.peer_address,
    });
    let object = body
        .as_object_mut()
        .unwrap_or_else(|| panic!("case {:?}: the request body must be an object", case.name));
    if let Some(address) = case.core_network_address {
        object.insert("CoreNetworkAddress".to_string(), json!(address));
    }
    if !case.inside_cidr_blocks.is_empty() {
        object.insert("InsideCidrBlocks".to_string(), json!(case.inside_cidr_blocks));
    }
    if let Some(asn) = case.peer_asn {
        object.insert("BgpOptions".to_string(), json!({ "PeerAsn": asn }));
    }
    if case.subnet {
        object.insert("SubnetArn".to_string(), json!(case_subnet(case)));
    }
    if !case.tags.is_empty() {
        let tags: Vec<Value> = case
            .tags
            .iter()
            .map(|(key, value)| tag(key, value))
            .collect();
        object.insert("Tags".to_string(), json!(tags));
    }
    body
}

/// `CreateConnectPeer`.
pub(crate) async fn create_connect_peer(emulator: &Emulator, body: Value) -> (u16, Value) {
    emulator.post("/connect-peers", body).await
}

/// `GetConnectPeer`, asserting the call succeeded.
async fn get_connect_peer(emulator: &Emulator, case: &str, id: &str) -> Value {
    let (status, body) = emulator.get(&format!("/connect-peers/{id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnectPeer expected 200, got {status} with {body}"
    );
    body["ConnectPeer"].clone()
}

/// The `ListConnectPeers` URI, carrying `query` (already escaped) when it is not empty.
fn list_uri(query: &str) -> String {
    if query.is_empty() {
        "/connect-peers".to_string()
    } else {
        format!("/connect-peers?{query}")
    }
}

/// `ListConnectPeers`, asserting the call succeeded.
async fn list(emulator: &Emulator, case: &str, query: &str) -> Value {
    let (status, body) = emulator.get(&list_uri(query)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConnectPeers expected 200, got {status} with {body}"
    );
    body
}

/// The summaries one page reports, in the order returned.
fn summaries(case: &str, body: &Value) -> Vec<Value> {
    body["ConnectPeers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectPeers array in {body}"))
        .clone()
}

/// The `ConnectPeerId`s one page reports, in the order returned.
fn reported(case: &str, body: &Value) -> Vec<String> {
    summaries(case, body)
        .iter()
        .map(|summary| {
            summary["ConnectPeerId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no ConnectPeerId in {summary}"))
                .to_string()
        })
        .collect()
}

/// The string member `member` of `value`, or `None` when the member is absent.
fn member(value: &Value, member: &str) -> Option<String> {
    value[member].as_str().map(str::to_string)
}

/// Create every row of the table against `subject`, asserting what the create answered, and
/// answer the minted peer ids in creation order.
async fn create_all(emulator: &Emulator, subject: &Subject) -> Vec<String> {
    let mut ids = Vec::new();
    for case in &PEER_CASES {
        let attachment = &subject.attachments[case.transport];
        let (status, created) =
            create_connect_peer(emulator, request_of(case, attachment)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateConnectPeer expected 200, got {status} with {created}",
            case.name
        );
        let peer = created["ConnectPeer"].clone();
        let id = member(&peer, "ConnectPeerId").unwrap_or_else(|| {
            panic!("case {:?}: no ConnectPeer.ConnectPeerId in {created}", case.name)
        });
        assert!(
            is_minted_id(&id, CONNECT_PEER),
            "case {:?}: expected a minted connect-peer id, got {id:?}",
            case.name
        );
        assert_peer(case, attachment, &peer, "CREATING");
        ids.push(id);
    }
    ids
}

/// Assert `peer` is the `ConnectPeer` the row `case` asks for, in state `state`.
///
/// Everything asserted here is computed from stored state: the core network, edge location
/// and protocol come from the attachment the peer hangs off (never from the request, which
/// names none of them), the core network ASN from the policy document's edge entry, and the
/// rest is what the create carried.
fn assert_peer(case: &PeerCase, attachment: &str, peer: &Value, state: &str) {
    let transport = &TRANSPORTS[case.transport];
    let name = case.name;
    assert_eq!(
        member(peer, "ConnectAttachmentId").as_deref(),
        Some(attachment),
        "case {name:?}: expected the peer to report its Connect attachment, got {peer}"
    );
    assert_eq!(
        member(peer, "EdgeLocation").as_deref(),
        Some(transport.region),
        "case {name:?}: expected the attachment's edge location, got {peer}"
    );
    assert_eq!(
        member(peer, "State").as_deref(),
        Some(state),
        "case {name:?}: expected state {state}, got {peer}"
    );
    assert!(
        peer["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
        "case {name:?}: expected a minted CreatedAt timestamp, got {peer}"
    );
    assert_eq!(
        member(peer, "SubnetArn"),
        case.subnet.then(|| case_subnet(case)),
        "case {name:?}: expected the subnet the create named, got {peer}"
    );
    assert_eq!(
        peer["LastModificationErrors"],
        json!([]),
        "case {name:?}: a peer this service accepted has no modification error, got {peer}"
    );

    let configuration = &peer["Configuration"];
    assert_eq!(
        member(configuration, "Protocol").as_deref(),
        Some(transport.protocol),
        "case {name:?}: expected the attachment's tunnel protocol, got {peer}"
    );
    assert_eq!(
        member(configuration, "PeerAddress").as_deref(),
        Some(case.peer_address),
        "case {name:?}: expected the peer address the create named, got {peer}"
    );
    assert_eq!(
        member(configuration, "CoreNetworkAddress").as_deref(),
        case.core_network_address,
        "case {name:?}: expected the core network address the create named, got {peer}"
    );
    assert_eq!(
        configuration["InsideCidrBlocks"],
        json!(case.inside_cidr_blocks),
        "case {name:?}: expected the inside CIDR blocks the create named, got {peer}"
    );

    let bgp = configuration["BgpConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name:?}: expected a BgpConfigurations array in {peer}"));
    assert_eq!(
        bgp.len(),
        1,
        "case {name:?}: one peer joins one edge to one appliance, so one BGP configuration, got {peer}"
    );
    let session = &bgp[0];
    assert_eq!(
        session["CoreNetworkAsn"].as_i64(),
        Some(edge_asn(transport.region)),
        "case {name:?}: expected the ASN the policy assigns this edge, got {peer}"
    );
    assert_eq!(
        session["PeerAsn"].as_i64(),
        case.peer_asn,
        "case {name:?}: expected the peer ASN the create named, got {peer}"
    );
    assert_eq!(
        member(session, "PeerAddress").as_deref(),
        Some(case.peer_address),
        "case {name:?}: expected the BGP session's peer address, got {peer}"
    );
    assert_eq!(
        member(session, "CoreNetworkAddress").as_deref(),
        case.core_network_address,
        "case {name:?}: expected the BGP session's core network address, got {peer}"
    );
}

/// Assert `summary` is the `ConnectPeerSummary` of the row `case`, in state `state`.
fn assert_summary(
    case: &PeerCase,
    core_network: &str,
    attachment: &str,
    summary: &Value,
    state: &str,
) {
    let transport = &TRANSPORTS[case.transport];
    let name = case.name;
    assert_eq!(
        member(summary, "CoreNetworkId").as_deref(),
        Some(core_network),
        "case {name:?}: expected the attachment's core network, got {summary}"
    );
    assert_eq!(
        member(summary, "ConnectAttachmentId").as_deref(),
        Some(attachment),
        "case {name:?}: expected the peer's Connect attachment, got {summary}"
    );
    assert_eq!(
        member(summary, "EdgeLocation").as_deref(),
        Some(transport.region),
        "case {name:?}: expected the attachment's edge location, got {summary}"
    );
    assert_eq!(
        member(summary, "ConnectPeerState").as_deref(),
        Some(state),
        "case {name:?}: expected state {state}, got {summary}"
    );
    assert_eq!(
        member(summary, "SubnetArn"),
        case.subnet.then(|| case_subnet(case)),
        "case {name:?}: expected the subnet the create named, got {summary}"
    );
    let tags: Vec<Value> = case
        .tags
        .iter()
        .map(|(key, value)| tag(key, value))
        .collect();
    assert_eq!(
        summary["Tags"],
        json!(tags),
        "case {name:?}: expected the tags the create carried, got {summary}"
    );
}

#[tokio::test]
async fn a_created_connect_peer_reads_back_through_get_and_list() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "peers", 0).await;
    let ids = create_all(&emulator, &subject).await;

    // Each row reads back through the reader of its own kind, and through the listing, from
    // the one stored record: nothing the create answered was invented for that response.
    for (case, id) in PEER_CASES.iter().zip(&ids) {
        let attachment = &subject.attachments[case.transport];
        let peer = get_connect_peer(&emulator, case.name, id).await;
        assert_eq!(
            member(&peer, "ConnectPeerId").as_deref(),
            Some(id.as_str()),
            "case {:?}: GetConnectPeer must report the peer addressed, got {peer}",
            case.name
        );
        assert_eq!(
            member(&peer, "CoreNetworkId").as_deref(),
            Some(subject.core_network.as_str()),
            "case {:?}: expected the attachment's core network, got {peer}",
            case.name
        );

        let body = list(&emulator, case.name, &format!("connectAttachmentId={attachment}")).await;
        let summary = summaries(case.name, &body)
            .into_iter()
            .find(|summary| member(summary, "ConnectPeerId").as_deref() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {:?}: the listing left out {id}", case.name));
        assert_summary(
            case,
            &subject.core_network,
            attachment,
            &summary,
            "CREATING",
        );

        // The tags the create carried are the ones the tagging slice reports for the peer's
        // minted ARN, so one stored list serves both readers.
        let (status, listed) = list_tags(&emulator, &expected_arn(CONNECT_PEER, id)).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListTagsForResource expected 200, got {status} with {listed}",
            case.name
        );
        let tags: Vec<Value> = case
            .tags
            .iter()
            .map(|(key, value)| tag(key, value))
            .collect();
        assert_eq!(
            listed,
            json!(tags),
            "case {:?}: expected the tags the create carried, got {listed}",
            case.name
        );
    }

    // The transition the clock owes: every peer is AVAILABLE once the settle span has
    // elapsed, in both readers, and no call was needed to move it.
    tokio::time::sleep(PAST_SETTLE).await;
    for (case, id) in PEER_CASES.iter().zip(&ids) {
        let attachment = &subject.attachments[case.transport];
        let peer = get_connect_peer(&emulator, case.name, id).await;
        assert_peer(case, attachment, &peer, "AVAILABLE");
        let body = list(&emulator, case.name, &format!("connectAttachmentId={attachment}")).await;
        let summary = summaries(case.name, &body)
            .into_iter()
            .find(|summary| member(summary, "ConnectPeerId").as_deref() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {:?}: the listing left out {id}", case.name));
        assert_summary(
            case,
            &subject.core_network,
            attachment,
            &summary,
            "AVAILABLE",
        );
    }
}

#[tokio::test]
async fn each_filter_narrows_the_listing_and_an_unknown_id_selects_nothing() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "filters", 0).await;
    let created = create_all(&emulator, &subject).await;
    let mut ids = created.clone();
    ids.sort();

    let body = list(&emulator, "the whole listing", "").await;
    assert_eq!(
        reported("the whole listing", &body),
        ids,
        "case {:?}: expected every stored peer in peer-id order, got {body}",
        "the whole listing"
    );
    assert!(
        body["NextToken"].is_null(),
        "case {:?}: a listing that fits one page carries no NextToken, got {body}",
        "the whole listing"
    );

    // The core network filter selects every peer of that core network — all of them here,
    // since one core network owns all four attachments.
    let case = "the core network filter";
    let body = list(
        &emulator,
        case,
        &format!("coreNetworkId={}", subject.core_network),
    )
    .await;
    assert_eq!(
        reported(case, &body),
        ids,
        "case {case:?}: expected every peer of that core network, got {body}"
    );

    // The attachment filter selects the peers of that attachment alone.
    for (index, attachment) in subject.attachments.iter().enumerate() {
        // What the table says this attachment carries, in the stable peer-id order.
        let mut wanted: Vec<String> = PEER_CASES
            .iter()
            .zip(&created)
            .filter(|(case, _)| case.transport == index)
            .map(|(_, id)| id.clone())
            .collect();
        wanted.sort();

        let body = list(
            &emulator,
            attachment,
            &format!("connectAttachmentId={attachment}"),
        )
        .await;
        assert_eq!(
            reported(attachment, &body),
            wanted,
            "case {attachment:?}: expected exactly this attachment's peers {wanted:?}, got {body}"
        );
    }

    // Both filters together select the peers that satisfy both.
    let case = "both filters together";
    let body = list(
        &emulator,
        case,
        &format!(
            "coreNetworkId={}&connectAttachmentId={}",
            subject.core_network, subject.attachments[0]
        ),
    )
    .await;
    assert_eq!(
        reported(case, &body).len(),
        PEER_CASES.iter().filter(|case| case.transport == 0).count(),
        "case {case:?}: expected the peers of that attachment in that core network, got {body}"
    );

    // Each row: a filter that is well-formed but names nothing this store holds. The
    // operation models no not-found failure, so each selects no peer rather than failing.
    for (case, query) in [
        (
            "a core network id nothing is stored under",
            "coreNetworkId=core-network-00000000000000099".to_string(),
        ),
        (
            "an attachment id nothing is stored under",
            "connectAttachmentId=attachment-00000000000000099".to_string(),
        ),
        (
            "a core network and an attachment that do not belong together",
            format!(
                "coreNetworkId=core-network-00000000000000099&connectAttachmentId={}",
                subject.attachments[0]
            ),
        ),
    ] {
        let body = list(&emulator, case, &query).await;
        assert_eq!(
            reported(case, &body),
            Vec::<String>::new(),
            "case {case:?}: expected no peer, got {body}"
        );
    }
}

#[tokio::test]
async fn one_core_networks_peers_are_not_reported_by_another() {
    let emulator = Emulator::new();
    // A core network lives in its own global network (one core network per global network),
    // so the second subject is built the same way as the first.
    let first = subject(&emulator, "first", 0).await;
    let mut first_ids = create_all(&emulator, &first).await;
    first_ids.sort();
    let second = subject(&emulator, "second", 100).await;
    let mut second_ids = create_all(&emulator, &second).await;
    second_ids.sort();

    for (case, core_network, wanted) in [
        ("the first core network", &first.core_network, &first_ids),
        ("the second core network", &second.core_network, &second_ids),
    ] {
        let body = list(&emulator, case, &format!("coreNetworkId={core_network}")).await;
        assert_eq!(
            &reported(case, &body),
            wanted,
            "case {case:?}: a core network reports only its own peers, got {body}"
        );
    }

    // Unfiltered, the listing spans both: a peer is never hidden from a listing that asked
    // for every one of them.
    let case = "the whole listing";
    let mut both = first_ids.clone();
    both.extend(second_ids.clone());
    both.sort();
    assert_eq!(
        reported(case, &list(&emulator, case, "").await),
        both,
        "case {case:?}: expected every peer of both core networks"
    );
}

#[tokio::test]
async fn every_page_size_walks_the_listing_exactly_once() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "pages", 0).await;
    let mut wanted = create_all(&emulator, &subject).await;
    wanted.sort();
    assert!(
        wanted.len() > 2,
        "the table must hold enough peers to page over, got {wanted:?}"
    );

    // Each row: a page size. 1 is the modelled lower bound, 500 the upper one, and the
    // sizes between them straddle the number of entries there are to report.
    for size in [1, 2, 3, wanted.len(), wanted.len() + 1, MAX_PAGE] {
        let case = format!("maxResults={size}");
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("maxResults={size}");
        let mut pages = 0usize;
        loop {
            let body = list(&emulator, &case, &query).await;
            let page = reported(&case, &body);
            assert!(
                !page.is_empty() && page.len() <= size,
                "case {case:?}: a page holds at least one and at most {size} entries, got {body}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= wanted.len(),
                "case {case:?}: paging must terminate within {} pages, got {body}",
                wanted.len()
            );
            match body["NextToken"].as_str() {
                Some(token) => query = format!("maxResults={size}&nextToken={token}"),
                None => break,
            }
        }
        assert_eq!(
            walked, wanted,
            "case {case:?}: following the tokens must report every peer exactly once"
        );
        let expected_pages = wanted.len().div_ceil(size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {} entries at {size} per page is {expected_pages} pages, walked {pages}",
            wanted.len()
        );
    }
}

#[tokio::test]
async fn a_read_reports_the_same_peers_to_a_later_caller() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "repeat", 0).await;
    let mut ids = create_all(&emulator, &subject).await;
    ids.sort();
    // Past the settle span, so the state the readers report is the terminal one and the
    // comparison below is of two answers that must be identical.
    tokio::time::sleep(PAST_SETTLE).await;

    let first = list(&emulator, "the first read", "").await;
    let first_peer = get_connect_peer(&emulator, "the first read", &ids[0]).await;
    for pass in ["the second read", "the third read"] {
        let body = list(&emulator, pass, "").await;
        assert_eq!(
            body, first,
            "case {pass:?}: a read must not change what a later caller sees"
        );
        let peer = get_connect_peer(&emulator, pass, &ids[0]).await;
        assert_eq!(
            peer, first_peer,
            "case {pass:?}: a read must not change the peer a later caller reads"
        );
    }
}

#[tokio::test]
async fn the_peers_of_a_global_network_are_in_its_network_resource_inventory() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "inventory", 0).await;
    let mut ids = create_all(&emulator, &subject).await;
    ids.sort();

    // The inventory reports the kind under the type name the member's own documentation
    // lists (`connect-peer`, `smithy_query query="doc"
    // shape="GetNetworkResourceCountsRequest" member="ResourceType"`).
    let case = "the connect-peer count";
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{}/network-resource-count?resourceType=connect-peer",
            subject.global_network
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetNetworkResourceCounts expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["NetworkResourceCounts"],
        json!([{ "ResourceType": "connect-peer", "Count": ids.len() }]),
        "case {case:?}: expected the {} peers the store holds, got {body}",
        ids.len()
    );

    let case = "the connect-peer resources";
    let (status, body) = emulator
        .get(&format!(
            "/global-networks/{}/network-resources?resourceType=connect-peer",
            subject.global_network
        ))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetNetworkResources expected 200, got {status} with {body}"
    );
    let listed: Vec<String> = body["NetworkResources"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a NetworkResources array in {body}"))
        .iter()
        .map(|entry| {
            member(entry, "ResourceId")
                .unwrap_or_else(|| panic!("case {case:?}: no ResourceId in {entry}"))
        })
        .collect();
    assert_eq!(
        listed, ids,
        "case {case:?}: expected every stored peer in the inventory, got {body}"
    );
}

#[tokio::test]
async fn a_repeated_client_token_answers_the_first_peer_and_refuses_a_different_request() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "idempotency", 0).await;
    let attachment = subject.attachments[0].clone();
    let body = json!({
        "ConnectAttachmentId": attachment,
        "PeerAddress": "203.0.113.40",
        "CoreNetworkAddress": "203.0.113.41",
        "InsideCidrBlocks": ["169.254.20.0/29"],
        "BgpOptions": { "PeerAsn": 65010 },
        "ClientToken": "token-one",
    });

    let (status, first) = create_connect_peer(&emulator, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {:?}: the first create expected 200, got {status} with {first}",
        "the first create"
    );
    let id = member(&first["ConnectPeer"], "ConnectPeerId")
        .unwrap_or_else(|| panic!("the first create reported no id: {first}"));

    let (status, retried) = create_connect_peer(&emulator, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {:?}: the retry expected 200, got {status} with {retried}",
        "the retry"
    );
    assert_eq!(
        member(&retried["ConnectPeer"], "ConnectPeerId").as_deref(),
        Some(id.as_str()),
        "case {:?}: a retry must answer the peer the first call minted, got {retried}",
        "the retry"
    );
    let listed = list(&emulator, "the retry", "").await;
    assert_eq!(
        reported("the retry", &listed),
        vec![id.clone()],
        "case {:?}: a retry must not mint a second peer, got {listed}",
        "the retry"
    );

    // Each row: the same token asking for something ELSE, which is a conflict rather than a
    // second peer.
    for (case, changed) in [
        ("a different peer address", json!("203.0.113.99")),
        ("a different core network address", json!("203.0.113.98")),
    ] {
        let mut different = body.clone();
        let object = different
            .as_object_mut()
            .unwrap_or_else(|| panic!("case {case:?}: the request body must be an object"));
        let member_name = if case.contains("core network") {
            "CoreNetworkAddress"
        } else {
            "PeerAddress"
        };
        object.insert(member_name.to_string(), changed);
        let (status, refused) = create_connect_peer(&emulator, different).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refused}"
        );
        let listed = list(&emulator, case, "").await;
        assert_eq!(
            reported(case, &listed),
            vec![id.clone()],
            "case {case:?}: a refused create stores nothing, got {listed}"
        );
    }
}

#[tokio::test]
async fn a_request_the_store_cannot_serve_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "refusals", 0).await;
    let connect = subject.attachments[0].clone();
    // An attachment of another kind in the same core network: a VPC attachment carries no
    // Connect peer, since a peer runs inside a Connect attachment's tunnel.
    let (status, created) = create_vpc_attachment(
        &emulator,
        json!({
            "CoreNetworkId": subject.core_network,
            "VpcArn": vpc_arn("us-west-2", 500),
            "SubnetArns": [subnet_arn("us-west-2", 500)],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateVpcAttachment expected 200, got {status} with {created}"
    );
    let vpc = attachment_id("the wrong attachment kind", &created["VpcAttachment"]);

    // Each row: a create that cannot be served, and the modelled refusal for it. Every
    // error named is one this operation declares (`smithy_query query="operation"
    // operation="CreateConnectPeer"`).
    for (case, body, status_wanted, code_wanted) in [
        (
            "an attachment id nothing is stored under",
            json!({
                "ConnectAttachmentId": "attachment-00000000000000099",
                "PeerAddress": "203.0.113.50",
            }),
            400,
            "ValidationException",
        ),
        (
            "an attachment of a kind that carries no Connect peer",
            json!({
                "ConnectAttachmentId": vpc,
                "PeerAddress": "203.0.113.54",
            }),
            400,
            "ValidationException",
        ),
        (
            "a subnet outside the Region the attachment's edge is in",
            json!({
                "ConnectAttachmentId": connect,
                "PeerAddress": "203.0.113.51",
                "SubnetArn": subnet_arn("eu-central-1", 777),
            }),
            400,
            "ValidationException",
        ),
        (
            "more inside CIDR blocks than one peer may name",
            json!({
                "ConnectAttachmentId": connect,
                "PeerAddress": "203.0.113.52",
                "InsideCidrBlocks": (0..=MAX_INSIDE_CIDR_BLOCKS)
                    .map(|index| format!("169.254.{index}.0/29"))
                    .collect::<Vec<String>>(),
            }),
            400,
            "ValidationException",
        ),
        (
            "an inside CIDR block named twice",
            json!({
                "ConnectAttachmentId": connect,
                "PeerAddress": "203.0.113.53",
                "InsideCidrBlocks": ["169.254.30.0/29", "169.254.30.0/29"],
            }),
            400,
            "ValidationException",
        ),
    ] {
        let (status, refused) = create_connect_peer(&emulator, body).await;
        assert_eq!(
            status, status_wanted,
            "case {case:?}: expected {status_wanted}, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            code_wanted,
            "case {case:?}: expected {code_wanted}, got {refused}"
        );
    }

    // Nothing above was stored: the listing is still empty.
    let case = "after every refusal";
    let body = list(&emulator, case, "").await;
    assert_eq!(
        reported(case, &body),
        Vec::<String>::new(),
        "case {case:?}: a refused create stores nothing, got {body}"
    );

    // Each row: a read that cannot be served.
    for (case, uri, status_wanted, code_wanted) in [
        (
            "a peer id nothing is stored under",
            "/connect-peers/connect-peer-00000000000000099".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a NextToken this service never issued",
            list_uri("nextToken=not-a-token"),
            400,
            "ValidationException",
        ),
        (
            "a NextToken that is hex but names no listing of this service",
            list_uri("nextToken=6162636465"),
            400,
            "ValidationException",
        ),
    ] {
        let (status, body) = emulator.get(&uri).await;
        assert_eq!(
            status, status_wanted,
            "case {case:?}: expected {status_wanted}, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            code_wanted,
            "case {case:?}: expected {code_wanted}, got {body}"
        );
    }
}
