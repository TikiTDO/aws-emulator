// SPDX-License-Identifier: CC0-1.0

//! `UpdateNode` — the settings of one MediaLive Anywhere node — end to end over
//! the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! RULE: `UpdateNode` changes the node its path names in the members the request
//! CARRIES — `Name`, `Role` and the whole `SdiSourceMappings` list — leaves every
//! member it does not carry exactly as it was, and reports the stored node; it
//! refuses a cluster/node pair that names no node a caller may still change
//! (`NotFoundException`, 404), a `Name` another live node of the same cluster
//! already holds (`ConflictException`, 409) and an SDI source mapping the node
//! cannot carry (`BadRequestException`, 400), writing nothing in each case.
//!
//! The suites are TABLES: the stub issue's single `UpdateNode` call is ONE row.
//!
//! A node lives IN a cluster and is addressed under it, so every suite starts by
//! creating one through `CreateCluster`; an SDI source mapping names an
//! SdiSource, so the suites that write one create it through `CreateSdiSource`.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{
    FIRST_SDI_CARD_NUMBER, MAX_NODE_SDI_SOURCE_MAPPINGS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::node_lifecycle::{cluster, created, delete_node, described, mapping, node_id, node_uri, ROLES};
use crate::sdi_source_lifecycle::{created as created_sdi_source, delete_source};
use crate::wire::{call, error_code, error_message, medialive};

/// The members of a node-shaped response `UpdateNode` never writes: the two the
/// create minted, the cluster the node belongs to, its lifecycle members and the
/// two lists derived from the rest of the store and from the create's own
/// request.
///
/// The complement of the three members this operation's request carries, so a
/// row that changes one of those asserts every other one is untouched.
const MEMBERS_UPDATE_NODE_NEVER_WRITES: [&str; 7] = [
    "id",
    "arn",
    "clusterId",
    "state",
    "connectionState",
    "channelPlacementGroups",
    "nodeInterfaceMappings",
];

/// `UpdateNode` on one node of one cluster (`smithy_query(query="operation",
/// operation="UpdateNode")`: `PUT /prod/clusters/{ClusterId}/nodes/{NodeId}`).
///
/// The one copy every suite that drives a node update, accepted or refused,
/// goes through.
pub(crate) async fn update_node(
    router: &MultiServiceRouter,
    cluster_id: &str,
    node: &str,
    body: Value,
) -> (u16, Value) {
    call(router, "PUT", &node_uri(cluster_id, node), Some(body)).await
}

/// One node, updated with the call asserted to have succeeded.
///
/// `201 Created` is the status the model gives this operation
/// (`smithy_query(query="operation", operation="UpdateNode")`: code 201), as it
/// does every other node operation that writes.
async fn updated(
    router: &MultiServiceRouter,
    case: &str,
    cluster_id: &str,
    node: &str,
    body: Value,
) -> Value {
    let (status, updated) = update_node(router, cluster_id, node, body.clone()).await;
    assert_eq!(
        status, 201,
        "case {case:?}: UpdateNode({body}): expected 201, got {status}: {updated}"
    );
    updated
}

/// One `sdiSourceMappings` entry, as the request shape carries it: all three of
/// the members `SdiSourceMappingUpdateRequest` documents as mandatory ("You must
/// specify all three parameters in this object").
///
/// Shared with the suite that pins `DeleteSdiSource`'s mapping precondition, so
/// both suites write a mapping the same way.
pub(crate) fn sdi_mapping(card_number: i64, channel_number: i64, sdi_source: &str) -> Value {
    json!({
        "cardNumber": card_number,
        "channelNumber": channel_number,
        "sdiSource": sdi_source,
    })
}

/// An `UpdateNode` body carrying exactly the members the row names — a member
/// the row leaves `None` is not in the request at all, which asks for no change
/// to it.
fn update_body(name: Option<&str>, role: Option<&str>, mappings: Option<Vec<Value>>) -> Value {
    let mut body = json!({});
    if let Some(name) = name {
        body["name"] = json!(name);
    }
    if let Some(role) = role {
        body["role"] = json!(role);
    }
    if let Some(mappings) = mappings {
        body["sdiSourceMappings"] = Value::Array(mappings);
    }
    body
}

/// `count` SdiSources created through `CreateSdiSource`, answering their ids —
/// the sources a mapping's `SdiSource` resolves against.
async fn sdi_sources(router: &MultiServiceRouter, case: &str, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for index in 0..count {
        let source = created_sdi_source(
            router,
            case,
            json!({ "name": format!("camera-{index}"), "type": "SINGLE" }),
        )
        .await;
        ids.push(
            source["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: CreateSdiSource reported no id: {source}"))
                .to_string(),
        );
    }
    ids
}

/// One update a row asks for: the members it carries, with each mapping naming
/// an SdiSource by its index in the row's created sources.
type Update = (Option<&'static str>, Option<&'static str>, Option<Vec<(i64, i64, usize)>>);

/// Rule: `UpdateNode` writes the members the request carries onto the node its
/// path names — `Name`, `Role` and the whole `SdiSourceMappings` list — leaves
/// every other member of that node, and every other node, exactly as it was, and
/// stores what it reports, so `DescribeNode` reports the same node from then on
/// and the clock owes nothing after the call.
///
/// The rows differ along which members the request carries (each one alone, all
/// three at once, none at all), along the `NodeRole` values ([`ROLES`]) and the
/// row that puts a role on a node that had none, along the mapping count (0, 1,
/// 2, 3), along the card and port numbers those mappings name, and along
/// updating the same node more than once — because that is what the rule
/// generalises over. Renaming is ONE of those rows, not the rule.
#[tokio::test]
async fn update_node_writes_the_settings_the_request_carries() {
    // (case, the body the node is created with, the updates asked for in order)
    let rows: Vec<(&str, Value, Vec<Update>)> = vec![
        (
            "a rename and nothing else",
            json!({ "name": "first", "role": ROLES[0] }),
            vec![(Some("renamed"), None, None)],
        ),
        (
            "the same name again",
            json!({ "name": "steady" }),
            vec![(Some("steady"), None, None)],
        ),
        (
            "a role onto a node that had none",
            json!({ "name": "roleless" }),
            vec![(None, Some(ROLES[1]), None)],
        ),
        (
            "a role changed, then changed back",
            json!({ "name": "flipping", "role": ROLES[0] }),
            vec![(None, Some(ROLES[1]), None), (None, Some(ROLES[0]), None)],
        ),
        (
            "one mapping onto a node that had none",
            json!({}),
            vec![(None, None, Some(vec![(FIRST_SDI_CARD_NUMBER.into(), 1, 0)]))],
        ),
        (
            "three ports across two cards",
            json!({ "name": "mapped", "role": ROLES[1] }),
            vec![(None, None, Some(vec![(1, 1, 0), (1, 2, 1), (2, 1, 2)]))],
        ),
        (
            "a mapping list replaced, then emptied",
            json!({ "name": "remapped" }),
            vec![
                (None, None, Some(vec![(1, 1, 0), (2, 3, 1)])),
                (None, None, Some(vec![(4, 2, 2)])),
                (None, None, Some(Vec::new())),
            ],
        ),
        (
            "every member at once",
            json!({
                "name": "complete",
                "role": ROLES[0],
                "nodeInterfaceMappings": [mapping("my-inputs", "eth1", "NAT")],
            }),
            vec![(Some("completed"), Some(ROLES[1]), Some(vec![(7, 4, 0)]))],
        ),
        (
            "a request that names no member at all",
            json!({ "name": "unasked", "role": ROLES[0] }),
            vec![(None, None, None)],
        ),
    ];

    for (case, create, updates) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "settings-cluster").await;
        let sources = sdi_sources(&router, case, 3).await;
        let node = created(&router, case, &cluster_id, create.clone()).await;
        let id = node_id(case, &node);
        // A bystander, so what the update touches is the node it names.
        let bystander = created(&router, case, &cluster_id, json!({ "name": "bystander" })).await;
        let bystander_id = node_id(case, &bystander);

        let mut expected = node.clone();
        for (name, role, mappings) in updates {
            let requested: Option<Vec<Value>> = mappings.map(|mappings| {
                mappings
                    .into_iter()
                    .map(|(card, channel, source)| {
                        sdi_mapping(card, channel, &sources[source])
                    })
                    .collect()
            });
            let body = update_body(name, role, requested.clone());
            let written = updated(&router, case, &cluster_id, &id, body.clone()).await;

            // A member the request carried is the node's now; one it did not
            // carry is what it already was.
            if let Some(name) = name {
                expected["name"] = json!(name);
            }
            if let Some(role) = role {
                expected["role"] = json!(role);
            }
            if let Some(requested) = requested {
                expected["sdiSourceMappings"] = Value::Array(requested);
            }

            for member in ["name", "role", "sdiSourceMappings"] {
                assert_eq!(
                    written[member], expected[member],
                    "case {case:?}: update {body}: expected {member:?} to be {:?}, got {:?}",
                    expected[member], written[member]
                );
            }
            for member in MEMBERS_UPDATE_NODE_NEVER_WRITES {
                assert_eq!(
                    written[member], node[member],
                    "case {case:?}: update {body}: expected {member:?} untouched, got {:?}",
                    written[member]
                );
            }

            // The write landed in the STORE, not just in the response.
            let read_back = described(&router, case, &cluster_id, &id).await;
            assert_eq!(
                read_back, written,
                "case {case:?}: update {body}: expected DescribeNode to report the stored node, got {read_back}"
            );

            // And the settings are in effect as of this call: the clock owes no
            // step after it.
            wait_for_the_clock();
            assert_eq!(
                described(&router, case, &cluster_id, &id).await,
                written,
                "case {case:?}: update {body}: expected the clock to owe nothing"
            );
        }

        assert_eq!(
            described(&router, case, &cluster_id, &bystander_id).await,
            bystander,
            "case {case:?}: expected the bystander node {bystander_id} to be untouched"
        );
    }
}

/// Rule: `UpdateNode` changes a node that EXISTS and that a caller may still
/// change, so a cluster/node pair naming no node of the account — and a node
/// whose deregistration `DeleteNode` has already accepted, which names no node a
/// caller may still change — is answered with `NotFoundException` (404, the error
/// `DescribeNode`, `DeleteNode` and `UpdateNodeState` model for the same node)
/// and writes nothing.
///
/// The rows differ along why the pair resolves to nothing — an id no node ever
/// had, a live node addressed under another cluster, a node inside its
/// deregistering window and one the clock has settled — and along which member
/// the refused request carried, because the refusal is about the node and not
/// about the settings.
#[tokio::test]
async fn update_node_refuses_a_node_it_cannot_change() {
    // (case, whether the id names a live node of another cluster, how far the
    // node's own deregistration has got: none / accepted / settled)
    let rows: [(&str, bool, Option<bool>); 4] = [
        ("an id no node ever had", false, None),
        ("a node of another cluster", true, None),
        ("inside the deregistering window", false, Some(false)),
        ("once deregistered", false, Some(true)),
    ];

    for (case, under_another_cluster, deregistration) in rows {
        for body in [
            json!({ "name": "renamed" }),
            json!({ "role": ROLES[1] }),
            json!({ "sdiSourceMappings": [] }),
            json!({}),
        ] {
            let router = medialive();
            let home = cluster(&router, "refusing-settings-cluster").await;
            let elsewhere = cluster(&router, "other-settings-cluster").await;
            let node = created(&router, case, &home, json!({ "name": "homed" })).await;
            let id = node_id(case, &node);

            // The pair the refused request names, and the message it must carry.
            let (cluster_id, asked, expected_message) = if under_another_cluster {
                (elsewhere.clone(), id.clone(), format!("Node {id} not found"))
            } else if deregistration.is_some() {
                (home.clone(), id.clone(), format!("Node {id} already deleted"))
            } else {
                // A well-formed node id no `CreateNode` minted.
                let unknown = "9999999".to_string();
                (
                    home.clone(),
                    unknown.clone(),
                    format!("Node {unknown} not found"),
                )
            };

            if let Some(settle) = deregistration {
                let (status, accepted) = delete_node(&router, &home, &id).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: DeleteNode: expected 202, got {status}: {accepted}"
                );
                if settle {
                    wait_for_the_clock();
                }
            }

            let before = described(&router, case, &home, &id).await;

            let (status, refusal) = update_node(&router, &cluster_id, &asked, body.clone()).await;
            assert_eq!(
                status, 404,
                "case {case:?}: UpdateNode({body}): expected 404, got {status}: {refusal}"
            );
            assert_eq!(
                error_code(status, &refusal),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {refusal}"
            );
            assert_eq!(
                error_message(&refusal),
                expected_message,
                "case {case:?}: expected the refusal to name the node it was given, got {refusal}"
            );

            // The refusal wrote nothing: the node is exactly as it was.
            assert_eq!(
                described(&router, case, &home, &id).await,
                before,
                "case {case:?}: update {body}: expected the refused update to leave the node as it was"
            );
        }
    }
}

/// Rule: a node's `Name` is "unique in the Cluster" and case-sensitive, so a
/// rename onto a name another LIVE node of the same cluster holds is refused
/// with `ConflictException` (409) and writes nothing — while a name no live node
/// of that cluster holds is accepted: the node's own name, a name held only in
/// another cluster, a name differing in case, and the name of a node whose
/// deregistration has been accepted.
///
/// The rows differ along where the name is already held and along how it differs
/// from the one held, because that is what the uniqueness generalises over.
#[tokio::test]
async fn update_node_keeps_a_name_unique_in_its_cluster() {
    // (case, the name the rename asks for, whether it is refused)
    let rows: [(&str, &str, bool); 5] = [
        ("the name a live sibling holds", "sibling", true),
        ("that name in another case", "SIBLING", false),
        ("a name only another cluster holds", "stranger", false),
        ("the node's own name", "renamer", false),
        ("a name nothing holds", "fresh", false),
    ];

    for (case, asked, refused) in rows {
        let router = medialive();
        let home = cluster(&router, "unique-name-cluster").await;
        let elsewhere = cluster(&router, "other-unique-name-cluster").await;

        let node = created(&router, case, &home, json!({ "name": "renamer" })).await;
        let id = node_id(case, &node);
        let sibling = created(&router, case, &home, json!({ "name": "sibling" })).await;
        let sibling_id = node_id(case, &sibling);
        // The same name, in the OTHER cluster: uniqueness is per cluster.
        created(&router, case, &elsewhere, json!({ "name": "stranger" })).await;

        if refused {
            let (status, refusal) =
                update_node(&router, &home, &id, json!({ "name": asked })).await;
            assert_eq!(
                status, 409,
                "case {case:?}: UpdateNode(name={asked:?}): expected 409, got {status}: {refusal}"
            );
            assert_eq!(
                error_code(status, &refusal),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {refusal}"
            );
            assert_eq!(
                error_message(&refusal),
                "Name already in use",
                "case {case:?}: expected the name clash to be reported, got {refusal}"
            );
            assert_eq!(
                described(&router, case, &home, &id).await,
                node,
                "case {case:?}: expected the refused rename to leave the node as it was"
            );
            assert_eq!(
                described(&router, case, &home, &sibling_id).await,
                sibling,
                "case {case:?}: expected the sibling that holds the name to be untouched"
            );

            // The name is free again once the sibling holding it is on its way
            // out of the account.
            let (status, accepted) = delete_node(&router, &home, &sibling_id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteNode(sibling): expected 202, got {status}: {accepted}"
            );
            let written = updated(&router, case, &home, &id, json!({ "name": asked })).await;
            assert_eq!(
                written["name"],
                json!(asked),
                "case {case:?}: expected the freed name to be accepted, got {:?}",
                written["name"]
            );
        } else {
            let written = updated(&router, case, &home, &id, json!({ "name": asked })).await;
            assert_eq!(
                written["name"],
                json!(asked),
                "case {case:?}: expected the rename to be accepted, got {:?}",
                written["name"]
            );
            assert_eq!(
                described(&router, case, &home, &id).await,
                written,
                "case {case:?}: expected DescribeNode to report the renamed node"
            );
        }
    }
}

/// Rule: an SDI source mapping connects one physical card and port to one
/// SdiSource this account holds, so every entry has to carry all three of the
/// members its shape documents as mandatory, name a card the hardware numbering
/// reaches (it "always starts at 1") and name a live SdiSource — and a list
/// longer than this emulator stores is out of bounds; each is refused with
/// `BadRequestException` (400) and writes nothing.
///
/// The rows differ along which of the three members is left out, along the card
/// number named (the published first card, one below it, one far below it, one
/// well above it), along why the `SdiSource` does not resolve (an id nothing
/// minted, a deleted source) and along the list length (the ceiling and one past
/// it), because that is what the rule generalises over.
#[tokio::test]
async fn update_node_refuses_an_sdi_source_mapping_it_cannot_store() {
    /// Which mapping list a row sends, built from the ids of the sources the row
    /// created: the live one, and the one it deleted.
    enum Mappings {
        /// One entry, as JSON, naming the live source where it names one at all.
        Entry(fn(&str) -> Value),
        /// One entry naming the DELETED source.
        OnDeletedSource,
        /// `count` well-formed entries on the live source.
        Repeated(usize),
    }

    // (case, the list, the status and message it is answered with)
    let rows: Vec<(String, Mappings, Option<String>)> = vec![
        (
            "no cardNumber".to_string(),
            Mappings::Entry(|source| json!({ "channelNumber": 1, "sdiSource": source })),
            Some(
                "every entry of sdiSourceMappings must specify cardNumber, channelNumber and sdiSource"
                    .to_string(),
            ),
        ),
        (
            "no channelNumber".to_string(),
            Mappings::Entry(|source| json!({ "cardNumber": 1, "sdiSource": source })),
            Some(
                "every entry of sdiSourceMappings must specify cardNumber, channelNumber and sdiSource"
                    .to_string(),
            ),
        ),
        (
            "no sdiSource".to_string(),
            Mappings::Entry(|_| json!({ "cardNumber": 1, "channelNumber": 1 })),
            Some(
                "every entry of sdiSourceMappings must specify cardNumber, channelNumber and sdiSource"
                    .to_string(),
            ),
        ),
        (
            "an empty sdiSource".to_string(),
            Mappings::Entry(|_| json!({ "cardNumber": 1, "channelNumber": 1, "sdiSource": "" })),
            // The member IS specified; what it names is no source the account
            // holds, which is the refusal an id nothing minted gets.
            Some(
                "sdiSourceMappings references SdiSource , which this account does not hold"
                    .to_string(),
            ),
        ),
        (
            // One below the first card the numbering uses.
            format!("cardNumber {}", FIRST_SDI_CARD_NUMBER - 1),
            Mappings::Entry(|source| sdi_mapping(i64::from(FIRST_SDI_CARD_NUMBER) - 1, 1, source)),
            Some(format!(
                "sdiSourceMappings names cardNumber {}; SDI card numbering starts at {FIRST_SDI_CARD_NUMBER}",
                FIRST_SDI_CARD_NUMBER - 1
            )),
        ),
        (
            "a negative cardNumber".to_string(),
            Mappings::Entry(|source| sdi_mapping(-7, 1, source)),
            Some(format!(
                "sdiSourceMappings names cardNumber -7; SDI card numbering starts at {FIRST_SDI_CARD_NUMBER}"
            )),
        ),
        (
            "an sdiSource id nothing minted".to_string(),
            Mappings::Entry(|_| sdi_mapping(1, 1, "8888888")),
            Some(
                "sdiSourceMappings references SdiSource 8888888, which this account does not hold"
                    .to_string(),
            ),
        ),
        (
            "a deleted sdiSource".to_string(),
            Mappings::OnDeletedSource,
            None, // the message names the deleted source's own id
        ),
        (
            // One past the ceiling this emulator stores.
            format!("{} entries", MAX_NODE_SDI_SOURCE_MAPPINGS + 1),
            Mappings::Repeated(MAX_NODE_SDI_SOURCE_MAPPINGS + 1),
            Some(format!(
                "sdiSourceMappings carries {} mappings; at most {MAX_NODE_SDI_SOURCE_MAPPINGS} are accepted",
                MAX_NODE_SDI_SOURCE_MAPPINGS + 1
            )),
        ),
    ];

    for (case, mappings, expected_message) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "mapping-cluster").await;
        let sources = sdi_sources(&router, &case, 2).await;
        let (live, doomed) = (sources[0].clone(), sources[1].clone());
        let (status, deleted) = delete_source(&router, &doomed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteSdiSource: expected 200, got {status}: {deleted}"
        );

        let node = created(&router, &case, &cluster_id, json!({ "name": "mapped" })).await;
        let id = node_id(&case, &node);

        let (list, expected_message) = match mappings {
            Mappings::Entry(entry) => (vec![entry(&live)], expected_message),
            Mappings::OnDeletedSource => (
                vec![sdi_mapping(1, 1, &doomed)],
                Some(format!(
                    "sdiSourceMappings references SdiSource {doomed}, which this account does not hold"
                )),
            ),
            Mappings::Repeated(count) => (
                (0..count)
                    .map(|index| sdi_mapping(1, index as i64 + 1, &live))
                    .collect(),
                expected_message,
            ),
        };
        let body = json!({ "sdiSourceMappings": list });

        let (status, refusal) = update_node(&router, &cluster_id, &id, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: UpdateNode({body}): expected 400, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {refusal}"
        );
        let expected_message = expected_message
            .unwrap_or_else(|| panic!("case {case:?}: this row names no expected message"));
        assert_eq!(
            error_message(&refusal),
            expected_message,
            "case {case:?}: expected the refusal to name what the request must change, got {refusal}"
        );

        // The refusal wrote nothing.
        assert_eq!(
            described(&router, &case, &cluster_id, &id).await,
            node,
            "case {case:?}: expected the refused update to leave the node as it was"
        );

        // And a list the rule DOES allow still reaches the same node: the
        // published first card, and the ceiling itself.
        for (allowed_case, count) in [("one entry", 1), ("the ceiling", MAX_NODE_SDI_SOURCE_MAPPINGS)]
        {
            let list: Vec<Value> = (0..count)
                .map(|index| sdi_mapping(i64::from(FIRST_SDI_CARD_NUMBER), index as i64 + 1, &live))
                .collect();
            let written = updated(
                &router,
                &case,
                &cluster_id,
                &id,
                json!({ "sdiSourceMappings": list.clone() }),
            )
            .await;
            assert_eq!(
                written["sdiSourceMappings"],
                Value::Array(list),
                "case {case:?}: {allowed_case}: expected the stored mappings to be the ones asked for, got {:?}",
                written["sdiSourceMappings"]
            );
        }
    }
}
