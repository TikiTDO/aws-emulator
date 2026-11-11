// SPDX-License-Identifier: CC0-1.0

//! The data plane's `_cat` APIs: `GET /_cat/indices[/{index}]` and `GET /_cat/nodes`,
//! "the compact and aligned text (CAT) APIs" a client enumerates a cluster's indexes and
//! nodes with.
//!
//! Every column is computed from what the control plane and the writes persisted — the
//! index records a domain holds, the documents in them, and the `ClusterConfig` topology
//! [`crate::ops::describe_domain_nodes`] reports the nodes from — so the listing and the
//! operations that report the same facts cannot disagree. A column whose value is a
//! node's own process detail (its address, its heap, its load) is published nowhere on a
//! managed domain, so it is left out rather than invented.
//!
//! Both renderings the APIs answer in: the JSON array a request asking for
//! `format=json` gets, with every value a string as the `_cat` APIs report them, and
//! otherwise the aligned text a terminal reads — the rows in the same order and carrying
//! the same values, since it is one listing in two forms.

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use serde_json::{json, Map, Value};

use crate::state::{fgac, Domain};

use super::{
    assigned_replicas, data_nodes, engine_distribution, guarded, health_of, index_not_found,
    node_roles, ok, setting_number, stable_uuid, wrong_method, Request, NUMBER_OF_REPLICAS,
    NUMBER_OF_SHARDS,
};

/// The query parameter a request names the rendering it wants in, and the value that names
/// the JSON one.
const FORMAT: &str = "format";

/// See [`FORMAT`].
const FORMAT_JSON: &str = "json";

/// The query parameter a text rendering asks for a header row with.
const VERBOSE: &str = "v";

/// The status a `_cat/indices` row reports for an index this emulator holds: every index a
/// write or a create put here is open — nothing closes one — so the column reports the one
/// state an index of this cluster is in.
const INDEX_OPEN: &str = "open";

/// `GET /_cat/indices`, and the per-index form that filters the listing to one index.
pub(super) fn indices(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: Option<&str>,
) -> RtResponse {
    match request.method {
        "GET" | "HEAD" => guarded(domain, request, fgac::ACTION_INDICES_STATS, |domain| {
            let named = match index_name {
                None => domain.indexes.keys().cloned().collect(),
                Some(named) => {
                    let resolved = domain.resolved_indexes(named);
                    if resolved.is_empty() {
                        return index_not_found(named);
                    }
                    resolved
                }
            };
            let nodes = data_nodes(domain);
            let rows: Vec<Vec<(&str, String)>> = named
                .iter()
                .filter_map(|name| domain.indexes.get(name))
                .map(|index| {
                    let shards = setting_number(&index.schema, NUMBER_OF_SHARDS).unwrap_or(1);
                    let copies = setting_number(&index.schema, NUMBER_OF_REPLICAS).unwrap_or(1);
                    let unassigned = copies
                        .saturating_sub(assigned_replicas(copies, nodes))
                        .saturating_mul(shards);
                    vec![
                        ("health", health_of(unassigned).to_string()),
                        ("status", INDEX_OPEN.to_string()),
                        ("index", index.index_name.clone()),
                        (
                            "uuid",
                            stable_uuid(&format!("{}/{}", domain.arn, index.index_name)),
                        ),
                        ("pri", shards.to_string()),
                        ("rep", copies.to_string()),
                        ("docs.count", index.documents.len().to_string()),
                        // Nothing deletes a document in place here: a delete removes it,
                        // so no index holds a deleted document awaiting a merge.
                        ("docs.deleted", "0".to_string()),
                    ]
                })
                .collect();
            listing(request, &rows)
        }),
        _ => wrong_method(request, "GET, HEAD"),
    }
}

/// `GET /_cat/nodes` — one row per node the domain's cluster configuration provisions.
pub(super) fn nodes(domain: &mut Domain, request: &Request<'_>) -> RtResponse {
    match request.method {
        "GET" | "HEAD" => guarded(domain, request, fgac::ACTION_NODES_INFO, |domain| {
            let (_, version) = engine_distribution(&domain.engine_version);
            let dedicated_master = domain.cluster_config.dedicated_master_enabled == Some(true);
            let census = crate::ops::describe_domain_nodes::domain_nodes(domain);
            // The cluster manager is ELECTED among the nodes eligible for it, and the
            // census is ordered, so the first eligible node is the one the cluster
            // reports — the same node on every read.
            let elected = census
                .iter()
                .find(|node| {
                    node_roles(
                        node.node_type.as_deref().unwrap_or_default(),
                        dedicated_master,
                    )
                    .contains(&CLUSTER_MANAGER_ROLE)
                })
                .and_then(|node| node.node_id.clone());
            let rows: Vec<Vec<(&str, String)>> = census
                .iter()
                .filter_map(|node| {
                    let id = node.node_id.clone()?;
                    let roles = node_roles(
                        node.node_type.as_deref().unwrap_or_default(),
                        dedicated_master,
                    );
                    Some(vec![
                        ("name", id.clone()),
                        ("version", version.clone()),
                        // "node.role: the roles of the node", as the single-letter
                        // abbreviation of each role the engine's own listing carries.
                        ("node.role", role_letters(&roles)),
                        (
                            "cluster_manager",
                            if elected.as_deref() == Some(id.as_str()) {
                                ELECTED
                            } else {
                                NOT_ELECTED
                            }
                            .to_string(),
                        ),
                    ])
                })
                .collect();
            listing(request, &rows)
        }),
        _ => wrong_method(request, "GET, HEAD"),
    }
}

/// The role a cluster's manager is elected among the holders of.
const CLUSTER_MANAGER_ROLE: &str = "cluster_manager";

/// What the `cluster_manager` column reports for the elected node, and for every other.
const ELECTED: &str = "*";

/// See [`ELECTED`].
const NOT_ELECTED: &str = "-";

/// The letters a `node.role` column abbreviates a node's roles with: the one the engine
/// publishes for each role (`d` data, `i` ingest, `m` cluster manager, `w` warm), in the
/// order the column reports them, which is the letters' own.
fn role_letters(roles: &[&str]) -> String {
    let mut letters: Vec<char> = roles
        .iter()
        .filter_map(|role| match *role {
            "data" => Some('d'),
            "ingest" => Some('i'),
            CLUSTER_MANAGER_ROLE => Some('m'),
            "warm" => Some('w'),
            _ => None,
        })
        .collect();
    letters.sort_unstable();
    letters.dedup();
    letters.into_iter().collect()
}

/// One `_cat` listing, in the rendering the request asked for.
///
/// Every row carries the same columns in the same order, so the text form's header is the
/// first row's column names and each line is that row's values.
fn listing(request: &Request<'_>, rows: &[Vec<(&str, String)>]) -> RtResponse {
    if request.parameter(FORMAT).as_deref() == Some(FORMAT_JSON) {
        let reported: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut object = Map::new();
                for (column, value) in row {
                    object.insert((*column).to_string(), json!(value));
                }
                Value::Object(object)
            })
            .collect();
        return ok(&Value::Array(reported));
    }
    let mut lines: Vec<String> = Vec::new();
    // The columns are ALIGNED: each is as wide as the widest value in it (and its own name
    // where the request asked for the header), which is what makes the text form readable
    // as the aligned table the `_cat` APIs are named for.
    let mut widths: Vec<usize> = Vec::new();
    let verbose = request.parameter(VERBOSE).is_some();
    for row in rows {
        for (at, (column, value)) in row.iter().enumerate() {
            let width = if verbose {
                value.chars().count().max(column.chars().count())
            } else {
                value.chars().count()
            };
            match widths.get_mut(at) {
                Some(held) => *held = (*held).max(width),
                None => widths.push(width),
            }
        }
    }
    let padded = |cells: Vec<&str>| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(at, cell)| {
                let width = widths.get(at).copied().unwrap_or_default();
                let pad = width.saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(pad))
            })
            .collect::<Vec<String>>()
            .join(" ")
            .trim_end()
            .to_string()
    };
    if verbose {
        if let Some(first) = rows.first() {
            lines.push(padded(first.iter().map(|(column, _)| *column).collect()));
        }
    }
    for row in rows {
        lines.push(padded(row.iter().map(|(_, value)| value.as_str()).collect()));
    }
    let mut body = lines.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    text(&body)
}

/// A 200 answer carrying the aligned text a `_cat` request asked for, which is not JSON
/// and so is not rendered as it.
fn text(body: &str) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(200).unwrap_or_else(|_| super::fallback_status()),
        SdkBody::from(body.as_bytes().to_vec()),
    );
    if let Err(refused) = response
        .headers_mut()
        .try_insert("content-type", "text/plain; charset=UTF-8")
    {
        ::tracing::warn!("the content type of a _cat answer could not be set: {refused}");
    }
    response
}
