// SPDX-License-Identifier: CC0-1.0

//! What every delete ABOVE a Connect peer in this service's resource tree does while the
//! peer is still stored, driven over HTTP through the real router (the harness lives in
//! `crate::global_network_lifecycle`, the Connect-peer subject and table in
//! `crate::connect_peer_lifecycle`).
//!
//! The rule under test: no delete of this service removes a resource that another stored
//! resource names as its only parent — the delete is refused with this service's dependency
//! `ValidationException` (HTTP 400), naming the resource being deleted and every dependent
//! kind still hanging off it, and nothing is removed — so a Connect peer can never be left
//! naming a Connect attachment or a core network no read of the service resolves. A Connect
//! peer hangs off the Connect attachment it is carried over
//! (`ConnectPeer$ConnectAttachmentId`, `smithy_query shape=ConnectPeer`), that Connect
//! attachment hangs off the attachment it is carried over
//! (`ConnectAttachment$TransportAttachmentId`), and both attachments hang off the core
//! network, so the whole chain has exactly one order that empties it: peers, then the Connect
//! attachment, then its transport, then the core network. Every earlier step is refused, and
//! once the chain is empty neither `GetConnectPeer` nor `ListConnectPeers` reports a peer.
//!
//! The tables span that class: rows differing in the tunnel protocol and edge location of the
//! Connect attachment carrying the peers (GRE and NO_ENCAP, three Regions), in how many peers
//! ride one attachment, in the address family and shape of each peer (both tunnel addresses,
//! none, IPv6, a subnet, no subnet), and — in the second test — in the attachment emptied
//! first, so the refusal is shown to track each attachment's OWN dependents rather than the
//! store as a whole. Nothing reaches into `crate::state`: every resource is created, read
//! back and removed through an operation.

use serde_json::{json, Value};

use crate::connect_peer_lifecycle::{
    create_connect_peer, request_of, subject, PEER_CASES, TRANSPORTS,
};
use crate::global_network_lifecycle::{error_code, Emulator};

/// How a dependency refusal names an attachment as the resource being deleted: the ARN
/// resource type (`attachment`) read as a title.
const ATTACHMENT_TITLE: &str = "Attachment";

/// How a dependency refusal names a core network as the resource being deleted.
const CORE_NETWORK_TITLE: &str = "Core Network";

/// How a refusal spells the blocking kinds this suite can be blocked by: the ARN resource
/// type read as words (`connect-peer` reads "connect peer").
const CONNECT_PEER: &str = "connect peer";
/// See [`CONNECT_PEER`].
const ATTACHMENT: &str = "attachment";

/// The `AttachmentState` a removed record reports (`smithy_query shape=AttachmentState`).
const DELETING: &str = "DELETING";

/// Assert `body` is this service's dependency refusal for `parent_title` blocked by exactly
/// `blocking`, the one kind each delete of the chain is blocked by.
///
/// The envelope and the kinds named go through the crate's ONE dependency-refusal assertion
/// (`crate::delete_dependency_refusal::assert_dependency_refusal`: HTTP 400,
/// `ValidationException`, the modelled catch-all reason and no field at fault, since no input
/// field is wrong). What this adds is the WHOLE message, so a refusal cannot start naming a
/// second kind — the thing that tells "blocked by the peers it carries" from "blocked by the
/// attachment standing on it" — without a row noticing.
fn assert_dependency_refusal(
    case: &str,
    parent_title: &str,
    blocking: &str,
    status: u16,
    body: &Value,
) {
    crate::delete_dependency_refusal::assert_dependency_refusal(
        case,
        parent_title,
        status,
        body,
        &[blocking],
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(
            format!(
                "{parent_title} cannot be deleted due to an existing {blocking} associated with \
                 it."
            )
            .as_str()
        ),
        "case {case:?}: expected the refusal to name {parent_title:?} and {blocking:?} alone, \
         got {body}"
    );
}

/// `DeleteAttachment`, the one delete of every attachment kind.
async fn delete_attachment(emulator: &Emulator, attachment: &str) -> (u16, Value) {
    emulator.delete(&format!("/attachments/{attachment}")).await
}

/// `DeleteAttachment` that must succeed, reporting the removed record as `DELETING`.
async fn delete_attachment_ok(emulator: &Emulator, case: &str, attachment: &str) {
    let (status, body) = delete_attachment(emulator, attachment).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteAttachment on {attachment} expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["Attachment"]["State"],
        json!(DELETING),
        "case {case:?}: the removed record reports {DELETING}, got {body}"
    );
}

/// `DeleteConnectPeer` that must succeed.
async fn delete_connect_peer_ok(emulator: &Emulator, case: &str, peer: &str) {
    let (status, body) = emulator.delete(&format!("/connect-peers/{peer}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteConnectPeer on {peer} expected 200, got {status} with {body}"
    );
}

/// `DeleteCoreNetwork`.
async fn delete_core_network(emulator: &Emulator, core_network: &str) -> (u16, Value) {
    emulator
        .delete(&format!("/core-networks/{core_network}"))
        .await
}

/// `GetConnectPeer`.
async fn get_connect_peer(emulator: &Emulator, peer: &str) -> (u16, Value) {
    emulator.get(&format!("/connect-peers/{peer}")).await
}

/// The attachment a Connect attachment is carried over, read back through
/// `GetConnectAttachment` (`ConnectAttachment$TransportAttachmentId`, `smithy_query
/// shape=ConnectAttachment`) rather than remembered from the create.
async fn transport_of(emulator: &Emulator, case: &str, attachment: &str) -> String {
    let (status, body) = emulator
        .get(&format!("/connect-attachments/{attachment}"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConnectAttachment on {attachment} expected 200, got {status} with \
         {body}"
    );
    body["ConnectAttachment"]["TransportAttachmentId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no TransportAttachmentId in {body}"))
        .to_string()
}

/// The `ConnectPeerId` a create answered.
fn peer_id(case: &str, created: &Value) -> String {
    created["ConnectPeer"]["ConnectPeerId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ConnectPeer.ConnectPeerId in {created}"))
        .to_string()
}

/// The `ConnectPeerId`s `ListConnectPeers` reports for `query` (already escaped), in the
/// order returned — the stable peer-id order of the store.
async fn listed(emulator: &Emulator, case: &str, query: &str) -> Vec<String> {
    let uri = if query.is_empty() {
        "/connect-peers".to_string()
    } else {
        format!("/connect-peers?{query}")
    };
    let (status, body) = emulator.get(&uri).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConnectPeers expected 200, got {status} with {body}"
    );
    body["ConnectPeers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConnectPeers array in {body}"))
        .iter()
        .map(|summary| {
            summary["ConnectPeerId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no ConnectPeerId in {summary}"))
                .to_string()
        })
        .collect()
}

/// One row of the chain table: which Connect attachment of the subject carries the peers, and
/// which rows of [`PEER_CASES`] ride it.
struct ChainCase {
    /// Names the case in every assertion message.
    name: &'static str,
    /// Index into `crate::connect_peer_lifecycle::TRANSPORTS` (and so into the subject's
    /// Connect attachments): the attachment under test.
    transport: usize,
    /// Indices into `PEER_CASES`; every row named here must ride `transport`.
    peers: &'static [usize],
}

/// The rows differ in the protocol and edge location of the attachment carrying the peers, in
/// how many peers ride it, and in the shape of each peer — everything the rule generalises
/// over. The last row carries no peer at all, so it is the control: the step the other rows
/// are refused at succeeds for it.
const CHAIN_CASES: [ChainCase; 5] = [
    ChainCase {
        name: "one GRE peer with both tunnel addresses",
        transport: 0,
        peers: &[0],
    },
    ChainCase {
        name: "two GRE peers on one attachment",
        transport: 0,
        peers: &[0, 2],
    },
    ChainCase {
        name: "two GRE peers, one naming no core network address and one on IPv6 ends",
        transport: 1,
        peers: &[1, 3],
    },
    ChainCase {
        name: "one NO_ENCAP peer reaching its appliance through a subnet",
        transport: 2,
        peers: &[4],
    },
    ChainCase {
        name: "no peer at all",
        transport: 3,
        peers: &[],
    },
];

#[tokio::test]
async fn no_delete_above_a_connect_peer_removes_the_resource_the_peer_names() {
    for case in CHAIN_CASES.iter() {
        let emulator = Emulator::new();
        let name = case.name;
        let subject = subject(&emulator, name, 0).await;
        let attachment = subject.attachments[case.transport].clone();
        let transport = transport_of(&emulator, name, &attachment).await;

        let mut peers = Vec::new();
        for row in case.peers {
            let peer_case = &PEER_CASES[*row];
            assert_eq!(
                peer_case.transport, case.transport,
                "case {name:?}: every peer of a row must ride the attachment under test"
            );
            let (status, created) =
                create_connect_peer(&emulator, request_of(peer_case, &attachment)).await;
            assert_eq!(
                status, 200,
                "case {name:?}: CreateConnectPeer expected 200, got {status} with {created}"
            );
            peers.push(peer_id(name, &created));
        }
        peers.sort();

        if peers.is_empty() {
            // The control row: with nothing carried over it, the very step the other rows
            // are refused at succeeds, so the refusal tracks stored dependents and not the
            // shape of the request.
            delete_attachment_ok(&emulator, name, &attachment).await;
            delete_attachment_ok(&emulator, name, &transport).await;
            continue;
        }

        // Step 1 of the chain: the Connect attachment the peers are carried over.
        let (status, body) = delete_attachment(&emulator, &attachment).await;
        assert_dependency_refusal(name, ATTACHMENT_TITLE, CONNECT_PEER, status, &body);

        // Step 2: the attachment that Connect attachment is itself carried over. Its one
        // dependent is of the attachment kind, so the refusal names that kind and not the
        // peers two levels below it.
        let (status, body) = delete_attachment(&emulator, &transport).await;
        assert_dependency_refusal(name, ATTACHMENT_TITLE, ATTACHMENT, status, &body);

        // Step 3: the core network both attachments hang off.
        let (status, body) = delete_core_network(&emulator, &subject.core_network).await;
        assert_dependency_refusal(name, CORE_NETWORK_TITLE, ATTACHMENT, status, &body);

        // None of the three refusals changed anything: every peer is still readable, still
        // names the attachment it rides and the core network that attachment hangs off, and
        // both listings still report it.
        for peer in &peers {
            let (status, read) = get_connect_peer(&emulator, peer).await;
            assert_eq!(
                status, 200,
                "case {name:?}: a refused delete must leave {peer} readable, got {status} with \
                 {read}"
            );
            assert_eq!(
                read["ConnectPeer"]["ConnectAttachmentId"],
                json!(attachment),
                "case {name:?}: {peer} must still name the attachment carrying it, got {read}"
            );
            assert_eq!(
                read["ConnectPeer"]["CoreNetworkId"],
                json!(subject.core_network),
                "case {name:?}: {peer} must still name its core network, got {read}"
            );
        }
        assert_eq!(
            listed(
                &emulator,
                name,
                &format!("connectAttachmentId={attachment}")
            )
            .await,
            peers,
            "case {name:?}: expected exactly {peers:?} reported for the attachment"
        );
        assert_eq!(
            listed(
                &emulator,
                name,
                &format!("coreNetworkId={}", subject.core_network)
            )
            .await,
            peers,
            "case {name:?}: expected exactly {peers:?} reported for the core network"
        );
        let (status, read) = emulator
            .get(&format!("/connect-attachments/{attachment}"))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a refused delete must leave {attachment} readable, got {status} with \
             {read}"
        );

        // Emptied from the bottom up, the same calls succeed in turn: the peers, then the
        // Connect attachment they rode, then the attachment that carried it.
        for peer in &peers {
            delete_connect_peer_ok(&emulator, name, peer).await;
        }
        delete_attachment_ok(&emulator, name, &attachment).await;
        delete_attachment_ok(&emulator, name, &transport).await;

        // And nothing is left naming a resource that is gone: the peers are not found by
        // their own reader, and neither listing reports one.
        for peer in &peers {
            let (status, gone) = get_connect_peer(&emulator, peer).await;
            assert_eq!(
                status, 404,
                "case {name:?}: GetConnectPeer on the removed {peer} expected 404, got {status} \
                 with {gone}"
            );
            assert_eq!(
                error_code(&gone),
                "ResourceNotFoundException",
                "case {name:?}: expected ResourceNotFoundException for {peer}, got {gone}"
            );
        }
        assert_eq!(
            listed(
                &emulator,
                name,
                &format!("coreNetworkId={}", subject.core_network)
            )
            .await,
            Vec::<String>::new(),
            "case {name:?}: the core network must report no peer once none is stored"
        );
        assert_eq!(
            listed(
                &emulator,
                name,
                &format!("connectAttachmentId={attachment}")
            )
            .await,
            Vec::<String>::new(),
            "case {name:?}: the removed attachment must report no peer"
        );
        let (status, gone) = emulator
            .get(&format!("/connect-attachments/{attachment}"))
            .await;
        assert_eq!(
            status, 404,
            "case {name:?}: GetConnectAttachment on the removed {attachment} expected 404, got \
             {status} with {gone}"
        );
    }
}

#[tokio::test]
async fn each_attachment_blocks_on_its_own_peers_until_the_whole_tree_is_empty() {
    let emulator = Emulator::new();
    let subject = subject(&emulator, "teardown order", 0).await;

    // Every row of the peer table, on the Connect attachment its transport names, so each of
    // the subject's four attachments carries at least one peer.
    let mut created: Vec<(usize, String)> = Vec::new();
    for case in PEER_CASES.iter() {
        let attachment = &subject.attachments[case.transport];
        let (status, body) = create_connect_peer(&emulator, request_of(case, attachment)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateConnectPeer expected 200, got {status} with {body}",
            case.name
        );
        created.push((case.transport, peer_id(case.name, &body)));
    }
    let mut every_peer: Vec<String> = created.iter().map(|(_, peer)| peer.clone()).collect();
    every_peer.sort();

    let mut transports = Vec::new();
    for attachment in &subject.attachments {
        transports.push(transport_of(&emulator, "teardown order", attachment).await);
    }

    let mut still_stored = every_peer.clone();
    for (index, attachment) in subject.attachments.iter().enumerate() {
        let case = format!(
            "the {} attachment at {}",
            TRANSPORTS[index].region, index
        );
        let mut mine: Vec<String> = created
            .iter()
            .filter(|(transport, _)| *transport == index)
            .map(|(_, peer)| peer.clone())
            .collect();
        mine.sort();
        assert!(
            !mine.is_empty(),
            "case {case:?}: every attachment of the subject carries a peer, so this one must"
        );

        // Its own peers block it, whatever the other attachments still carry or have
        // already had removed.
        let (status, body) = delete_attachment(&emulator, attachment).await;
        assert_dependency_refusal(&case, ATTACHMENT_TITLE, CONNECT_PEER, status, &body);

        for peer in &mine {
            delete_connect_peer_ok(&emulator, &case, peer).await;
            still_stored.retain(|held| held != peer);
        }
        delete_attachment_ok(&emulator, &case, attachment).await;
        delete_attachment_ok(&emulator, &case, &transports[index]).await;

        // The peers of the attachments not reached yet are untouched: removing one
        // attachment's dependents never reaches another's.
        assert_eq!(
            listed(
                &emulator,
                &case,
                &format!("coreNetworkId={}", subject.core_network)
            )
            .await,
            still_stored,
            "case {case:?}: expected exactly {still_stored:?} still stored"
        );

        // While ANY attachment is left the core network keeps refusing.
        if index + 1 < subject.attachments.len() {
            let (status, body) = delete_core_network(&emulator, &subject.core_network).await;
            assert_dependency_refusal(&case, CORE_NETWORK_TITLE, ATTACHMENT, status, &body);
        }
    }

    assert!(
        still_stored.is_empty(),
        "every peer of the subject was removed with its attachment, so none is left: \
         {still_stored:?}"
    );

    // With the tree empty the core network deletes, and no peer is left naming it.
    let (status, body) = delete_core_network(&emulator, &subject.core_network).await;
    assert_eq!(
        status, 200,
        "an emptied core network must delete, got {status} with {body}"
    );
    assert_eq!(
        body["CoreNetwork"]["State"],
        json!(DELETING),
        "the removed core network reports {DELETING}, got {body}"
    );
    let (status, gone) = emulator
        .get(&format!("/core-networks/{}", subject.core_network))
        .await;
    assert_eq!(
        status, 404,
        "GetCoreNetwork after the delete expected 404, got {status} with {gone}"
    );
    for peer in &every_peer {
        let (status, gone) = get_connect_peer(&emulator, peer).await;
        assert_eq!(
            status, 404,
            "{peer} must not outlive the tree it hung off, got {status} with {gone}"
        );
    }
    assert_eq!(
        listed(&emulator, "the empty store", "").await,
        Vec::<String>::new(),
        "no peer is stored once every attachment that carried one is gone"
    );
}
