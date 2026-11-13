// SPDX-License-Identifier: CC0-1.0

//! `DeleteSdiSource`'s whole documented precondition, end to end over the
//! restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! RULE: "Delete an SdiSource. The SdiSource must not be part of any
//! SidSourceMapping and must not be attached to any input"
//! (`smithy_query(query="doc", shape="DeleteSdiSource")`) is ONE precondition
//! over TWO stores, so the delete is the modelled `ConflictException` (409)
//! while EITHER referrer still names the source — an input that ingests from it
//! or a node whose `SdiSourceMappings` maps a card port to it — and writes
//! nothing; it succeeds, putting the source into the terminal `DELETED` state,
//! exactly once no live referrer of either kind names it.
//!
//! The suites are TABLES: the reported single-node case is ONE row.
//!
//! An SDI source mapping is created by UPDATING a node ("you create a mapping
//! for an SDI source by updating a Node"), and a node lives in a cluster, so
//! every row that maps a source goes through `CreateCluster`, `CreateNode` and
//! `UpdateNode`.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::FIRST_SDI_CARD_NUMBER;
use smithy_server_runtime::MultiServiceRouter;

use crate::node_lifecycle::{cluster, created as created_node, delete_node, described, node_id};
use crate::node_settings_update::{sdi_mapping, update_node};
use crate::sdi_source_lifecycle::{
    created as created_sdi_source, delete_source, describe_source,
};
use crate::wire::{call, error_code, error_message, medialive};

/// The message a delete refused because a node maps the source carries: the
/// mapping half of the documented precondition, parameterized by the request's
/// own `SdiSourceId` and by nothing else.
fn mapped_message(sdi_source_id: &str) -> String {
    format!("SdiSource {sdi_source_id} is part of an SdiSourceMapping and cannot be deleted")
}

/// The message a delete refused because an input is attached to the source
/// carries: the other half of the same precondition, parameterized the same way.
fn attached_message(sdi_source_id: &str) -> String {
    format!("SdiSource {sdi_source_id} is attached to an input and cannot be deleted")
}

/// One SdiSource created through `CreateSdiSource`, answering the `Id` it minted.
async fn source(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = created_sdi_source(router, case, json!({ "name": name, "type": "SINGLE" })).await;
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateSdiSource reported no id: {created}"))
        .to_string()
}

/// One node of `cluster_id`, created and then updated to carry `mappings` —
/// which is the only way a mapping comes to exist. Answers the node's `Id`.
async fn node_mapping(
    router: &MultiServiceRouter,
    case: &str,
    cluster_id: &str,
    name: &str,
    mappings: Vec<Value>,
) -> String {
    let node = created_node(router, case, cluster_id, json!({ "name": name })).await;
    let id = node_id(case, &node);
    write_mappings(router, case, cluster_id, &id, mappings).await;
    id
}

/// `UpdateNode` writing `mappings` as the node's whole mapping list, asserted to
/// have been accepted and to have stored exactly that list.
async fn write_mappings(
    router: &MultiServiceRouter,
    case: &str,
    cluster_id: &str,
    node: &str,
    mappings: Vec<Value>,
) {
    let body = json!({ "sdiSourceMappings": mappings.clone() });
    let (status, written) = update_node(router, cluster_id, node, body.clone()).await;
    assert_eq!(
        status, 201,
        "case {case:?}: UpdateNode({body}): expected 201, got {status}: {written}"
    );
    assert_eq!(
        written["sdiSourceMappings"],
        Value::Array(mappings),
        "case {case:?}: expected UpdateNode to store the mappings asked for, got {:?}",
        written["sdiSourceMappings"]
    );
}

/// One input of type `SDI` ingesting from `sdi_source`, asserted to have been
/// created — the other half of the precondition's referrer.
async fn attach_input(router: &MultiServiceRouter, case: &str, name: &str, sdi_source: &str) {
    let body = json!({ "name": name, "type": "SDI", "sdiSources": [sdi_source] });
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateInput: expected 201, got {status}: {response}"
    );
}

/// The source as `DescribeSdiSource` reports it, asserted readable: a source is
/// readable whether it is live or in the terminal `DELETED` state.
async fn read_back(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = describe_source(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeSdiSource({id}): expected 200, got {status}: {body}"
    );
    body
}

/// The `sdiSourceMappings` of one node, as `DescribeNode` reports them.
async fn mappings_of(
    router: &MultiServiceRouter,
    case: &str,
    cluster_id: &str,
    node: &str,
) -> Value {
    described(router, case, cluster_id, node).await["sdiSourceMappings"].clone()
}

/// Rule: while a LIVE node's `SdiSourceMappings` names the source, its delete is
/// the modelled `ConflictException` (409) carrying the mapping half of the
/// precondition, and nothing is written; once no node maps it any more, the same
/// delete succeeds.
///
/// The rows differ along how many nodes map the source, along how many clusters
/// those nodes are spread over, along which entry of a multi-entry list names it
/// and along the card/port the entry names — because that is what the rule
/// generalises over. The reported single-node case is the first row.
#[tokio::test]
async fn a_source_a_live_node_maps_cannot_be_deleted() {
    // (case, clusters, nodes per cluster, how many OTHER sources each list names)
    let rows: [(&str, usize, usize, usize); 5] = [
        ("one node, sole mapping", 1, 1, 0),
        ("one node, the second of two entries", 1, 1, 1),
        ("one node, the third of three entries", 1, 1, 2),
        ("two nodes of one cluster", 1, 2, 0),
        ("two clusters, one node each", 2, 1, 1),
    ];

    for (case, clusters, nodes_per_cluster, others) in rows {
        let router = medialive();
        let doomed = source(&router, case, "wired").await;
        // The other sources exist so the node's list is not just this one entry.
        let mut other_ids = Vec::with_capacity(others);
        for index in 0..others {
            other_ids.push(source(&router, case, &format!("other-{index}")).await);
        }

        // The mapping list every node of this row carries: the other sources
        // first, then the doomed one, each on its own port of the first card
        // the published numbering uses.
        let list: Vec<Value> = other_ids
            .iter()
            .chain(std::iter::once(&doomed))
            .enumerate()
            .map(|(port, id)| {
                sdi_mapping(
                    i64::from(FIRST_SDI_CARD_NUMBER),
                    i64::try_from(port).unwrap_or_default() + 1,
                    id,
                )
            })
            .collect();

        let mut mapped: Vec<(String, String)> = Vec::new();
        for cluster_index in 0..clusters {
            let cluster_id = cluster(&router, &format!("mapping-cluster-{cluster_index}")).await;
            for node_index in 0..nodes_per_cluster {
                let node = node_mapping(
                    &router,
                    case,
                    &cluster_id,
                    &format!("mapped-{cluster_index}-{node_index}"),
                    list.clone(),
                )
                .await;
                mapped.push((cluster_id.clone(), node));
            }
        }

        let before = read_back(&router, case, &doomed).await;
        let (status, refusal) = delete_source(&router, &doomed).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected deleting a mapped source to conflict, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            mapped_message(&doomed),
            "case {case:?}: expected the refusal to name the mapping half of the precondition, \
             got {refusal}"
        );

        // The refusal wrote nothing: the source is unchanged (so NOT `DELETED`),
        // and every node still reports the mapping it carried.
        let after = read_back(&router, case, &doomed).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused delete to change nothing, got {after}"
        );
        for (cluster_id, node) in &mapped {
            assert_eq!(
                mappings_of(&router, case, cluster_id, node).await,
                Value::Array(list.clone()),
                "case {case:?}: expected node {node} to keep its mappings"
            );
        }

        // Taking the source out of every node's list is what releases it: the
        // list the update carries is the node's WHOLE mapping list, so the
        // entries that stay are the other sources'.
        let kept: Vec<Value> = list
            .iter()
            .filter(|mapping| mapping["sdiSource"] != json!(doomed))
            .cloned()
            .collect();
        for (cluster_id, node) in &mapped {
            write_mappings(&router, case, cluster_id, node, kept.clone()).await;
        }

        let (status, deleted) = delete_source(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the released source to be deleted, got {status}: {deleted}"
        );
        assert_eq!(
            deleted["state"],
            json!("DELETED"),
            "case {case:?}: expected the deleted source to be DELETED, got {:?}",
            deleted["state"]
        );
        assert_eq!(
            read_back(&router, case, &doomed).await["state"],
            json!("DELETED"),
            "case {case:?}: expected DescribeSdiSource to report the source DELETED"
        );
    }
}

/// What a row puts in the account beside the source before its delete is driven
/// — every shape of node store that must leave the delete alone.
#[derive(Debug, Clone, Copy)]
enum Unmapped {
    /// The account holds no node at all.
    NoNode,
    /// A node that carries no mapping.
    NodeWithoutMappings,
    /// A node that maps a DIFFERENT source.
    NodeMappingAnotherSource,
    /// A node that mapped this source and then had the mapping replaced by
    /// another source's.
    MappingReplaced,
    /// A node that mapped this source and whose own delete has been accepted,
    /// so it is on its way out of the cluster and maps nothing.
    NodeDeleted,
}

/// Rule, the other side of it: a source NO live node maps and no input attaches
/// is deleted — 200, the terminal `DELETED` state — so the precondition refuses
/// only an actual reference.
///
/// The rows differ along why no live node maps the source: none exists, one
/// exists with no mappings, one maps another source, one had this source's
/// mapping replaced, one that mapped it is on its way out of the cluster.
#[tokio::test]
async fn a_source_no_live_node_maps_is_deleted() {
    let rows: [(&str, Unmapped); 5] = [
        ("no node at all", Unmapped::NoNode),
        ("a node with no mappings", Unmapped::NodeWithoutMappings),
        ("a node mapping another source", Unmapped::NodeMappingAnotherSource),
        ("a node whose mapping was replaced", Unmapped::MappingReplaced),
        ("a node whose delete was accepted", Unmapped::NodeDeleted),
    ];

    for (case, setup) in rows {
        let router = medialive();
        let doomed = source(&router, case, "wired").await;
        let other = source(&router, case, "other").await;
        let card = i64::from(FIRST_SDI_CARD_NUMBER);

        match setup {
            Unmapped::NoNode => {}
            Unmapped::NodeWithoutMappings => {
                let cluster_id = cluster(&router, "unmapped-cluster").await;
                node_mapping(&router, case, &cluster_id, "bare", Vec::new()).await;
            }
            Unmapped::NodeMappingAnotherSource => {
                let cluster_id = cluster(&router, "unmapped-cluster").await;
                node_mapping(
                    &router,
                    case,
                    &cluster_id,
                    "elsewhere",
                    vec![sdi_mapping(card, 1, &other)],
                )
                .await;
            }
            Unmapped::MappingReplaced => {
                let cluster_id = cluster(&router, "unmapped-cluster").await;
                let node = node_mapping(
                    &router,
                    case,
                    &cluster_id,
                    "replaced",
                    vec![sdi_mapping(card, 1, &doomed)],
                )
                .await;
                write_mappings(
                    &router,
                    case,
                    &cluster_id,
                    &node,
                    vec![sdi_mapping(card, 1, &other)],
                )
                .await;
            }
            Unmapped::NodeDeleted => {
                let cluster_id = cluster(&router, "unmapped-cluster").await;
                let node = node_mapping(
                    &router,
                    case,
                    &cluster_id,
                    "leaving",
                    vec![sdi_mapping(card, 1, &doomed)],
                )
                .await;
                let (status, accepted) = delete_node(&router, &cluster_id, &node).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: DeleteNode: expected 202, got {status}: {accepted}"
                );
            }
        }

        let (status, deleted) = delete_source(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an unmapped source to be deleted, got {status}: {deleted}"
        );
        assert_eq!(
            deleted["state"],
            json!("DELETED"),
            "case {case:?}: expected the deleted source to be DELETED, got {:?}",
            deleted["state"]
        );
        // And a repeat is still idempotent rather than a conflict.
        let (status, again) = delete_source(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeated DeleteSdiSource: expected 200, got {status}: {again}"
        );
        assert_eq!(
            again["state"],
            json!("DELETED"),
            "case {case:?}: repeated DeleteSdiSource: expected DELETED, got {:?}",
            again["state"]
        );
    }
}

/// Which referrers a row puts on the source before its delete is driven, and
/// which half of the precondition the refusal must then carry.
#[derive(Debug, Clone, Copy)]
enum Referrer {
    /// An input ingests from the source.
    Input,
    /// A node maps a card port to the source.
    NodeMapping,
    /// Both at once.
    Both,
}

/// Rule: each half of the one precondition is answered as the same modelled
/// `ConflictException`, carrying the message of the half the source fails and
/// naming the `SdiSourceId` the request supplied.
#[tokio::test]
async fn each_half_of_the_precondition_answers_a_conflict() {
    let rows: [(&str, Referrer); 3] = [
        ("an attached input", Referrer::Input),
        ("a node mapping", Referrer::NodeMapping),
        ("both at once", Referrer::Both),
    ];

    for (case, referrer) in rows {
        let router = medialive();
        let id = source(&router, case, "wired").await;
        let card = i64::from(FIRST_SDI_CARD_NUMBER);

        if matches!(referrer, Referrer::Input | Referrer::Both) {
            attach_input(&router, case, "sdi-input", &id).await;
        }
        if matches!(referrer, Referrer::NodeMapping | Referrer::Both) {
            let cluster_id = cluster(&router, "both-cluster").await;
            node_mapping(
                &router,
                case,
                &cluster_id,
                "mapped",
                vec![sdi_mapping(card, 1, &id)],
            )
            .await;
        }

        let (status, refusal) = delete_source(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected a conflict, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refusal}"
        );
        let message = error_message(&refusal);
        let expected: Vec<String> = match referrer {
            Referrer::Input => vec![attached_message(&id)],
            Referrer::NodeMapping => vec![mapped_message(&id)],
            // A source that fails BOTH halves is refused on one of them; which
            // one is not something the precondition settles.
            Referrer::Both => vec![attached_message(&id), mapped_message(&id)],
        };
        assert!(
            expected.contains(&message),
            "case {case:?}: expected one of {expected:?}, got {message:?}"
        );

        // The source is still there, in the state it was in.
        assert_ne!(
            read_back(&router, case, &id).await["state"],
            json!("DELETED"),
            "case {case:?}: expected the refused delete to leave the source live"
        );
    }
}
