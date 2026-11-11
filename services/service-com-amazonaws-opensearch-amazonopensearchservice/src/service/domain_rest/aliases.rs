// SPDX-License-Identifier: CC0-1.0

//! The data plane's ALIAS API: `POST /_aliases`, the alias actions a client rotates an
//! index set with, and the per-index forms `GET`/`PUT`/`DELETE /{index}/_alias[/{name}]`.
//!
//! An alias is "a virtual index name that can point to one or more indexes", so every
//! alias lives on the index it points at ([`state::IndexAlias`]): the actions API writes
//! those records, a read reports them beside the index's mappings and settings, and the
//! one resolution every route that reads or writes documents goes through
//! ([`Domain::resolved_indexes`], [`Domain::write_index`]) is what makes an alias behave
//! as the index name it stands for.

use std::collections::{BTreeMap, BTreeSet};

use aws_smithy_runtime_api::http::Response as RtResponse;
use serde_json::{json, Map, Value};

use crate::state::{self, fgac, ingest, Domain, IndexAlias};

use super::{error, guarded, index_not_found, ok, wrong_method, Request};

/// The member an alias action names the alias with, and the plural form beside it.
const ALIAS_MEMBERS: [&str; 2] = ["alias", "aliases"];

/// The member an alias action names the index with, and the plural form beside it.
const INDEX_MEMBERS: [&str; 2] = ["index", "indices"];

/// The member an `add` action marks the index a write through the alias lands in with.
const IS_WRITE_INDEX: &str = "is_write_index";

/// The engine's error type for a name no alias may carry.
const INVALID_ALIAS_NAME_EXCEPTION: &str = "invalid_alias_name_exception";

/// The engine's error type for an alias an index does not carry.
const ALIASES_NOT_FOUND_EXCEPTION: &str = "aliases_not_found_exception";

/// How many aliases ONE index may carry before an action is refused.
///
/// The emulator's OWN bound on the store, like [`state::INDEX_QUOTA`]: a caller looping
/// with fresh alias names must not grow this process's state without end.
pub(super) const ALIAS_QUOTA: usize = 100;

/// How many actions ONE `_aliases` request may carry.
///
/// The emulator's own bound on the work one request does: each action is applied to every
/// index it names, and the indexes a domain holds are bounded by [`state::INDEX_QUOTA`],
/// so this bounds the request itself.
const MAX_ALIAS_ACTIONS: usize = 100;

/// The most (index, alias) PAIRS one `_aliases` request may act on, over the whole
/// request.
///
/// The emulator's OWN bound on the work one request does, and the one that matters: an
/// action acts on every index it names times every alias it names, so bounding the actions
/// alone would leave their product to be chosen by the body. Past this the request is
/// refused before anything is written — the surface serves a request on the calling task
/// while it holds this service's store, so the work one request can ask for has to be a
/// constant and not a function of its size.
const MAX_ALIAS_PAIRS: usize = 1_000;

/// `POST /_aliases` — the alias actions, applied as one: "the operations are performed
/// atomically", so an action the cluster refuses leaves every alias as it was.
pub(super) fn alias_actions(domain: &mut Domain, request: &Request<'_>) -> RtResponse {
    match request.method {
        "POST" => guarded(domain, request, fgac::ACTION_ALIASES, |domain| {
            apply_actions(domain, request)
        }),
        // `GET /_aliases` is the cluster-wide listing, the same report the per-index form
        // answers for one index.
        "GET" => guarded(domain, request, fgac::ACTION_GET_ALIASES, |domain| {
            ok(&listing(domain, None, None))
        }),
        _ => wrong_method(request, "GET, POST"),
    }
}

/// `GET`, `PUT` or `DELETE` on the aliases of ONE index: the per-index spelling of the
/// same actions, where the path names the index and (for a write) the alias.
///
/// `index_name` is `None` for the cluster-wide `GET /_alias`, which reports the aliases of
/// every index — the same listing `GET /_aliases` gives; a WRITE naming no index names
/// nothing to write to.
pub(super) fn index_alias_request(
    domain: &mut Domain,
    request: &Request<'_>,
    index_name: Option<&str>,
    alias: Option<&str>,
) -> RtResponse {
    match request.method {
        "GET" | "HEAD" => guarded(domain, request, fgac::ACTION_GET_ALIASES, |domain| {
            if let Some(index_name) = index_name {
                if !domain.indexes.contains_key(index_name) {
                    return index_not_found(index_name);
                }
            }
            ok(&listing(domain, index_name, alias))
        }),
        "PUT" | "POST" => guarded(domain, request, fgac::ACTION_ALIASES, |domain| {
            let (Some(index_name), Some(alias)) = (index_name, alias) else {
                return missing(if index_name.is_none() {
                    INDEX_MEMBERS[0]
                } else {
                    ALIAS_MEMBERS[0]
                });
            };
            let body = match request.json() {
                Ok(body) => body.unwrap_or_else(|| json!({})),
                Err(refusal) => return *refusal,
            };
            match add(domain, index_name, alias, &body) {
                Ok(()) => ok(&json!({ "acknowledged": true })),
                Err(refusal) => *refusal,
            }
        }),
        "DELETE" => guarded(domain, request, fgac::ACTION_ALIASES, |domain| {
            let (Some(index_name), Some(alias)) = (index_name, alias) else {
                return missing(if index_name.is_none() {
                    INDEX_MEMBERS[0]
                } else {
                    ALIAS_MEMBERS[0]
                });
            };
            match remove(domain, index_name, alias) {
                Ok(()) => ok(&json!({ "acknowledged": true })),
                Err(refusal) => *refusal,
            }
        }),
        _ => wrong_method(request, "GET, HEAD, PUT, POST, DELETE"),
    }
}

/// Apply every action the request's `actions` list carries, after reading the whole list:
/// a refused action leaves the aliases as they were.
fn apply_actions(domain: &mut Domain, request: &Request<'_>) -> RtResponse {
    let body = match request.json() {
        Ok(Some(Value::Object(members))) => Value::Object(members),
        Ok(_) => return validation_failed("[actions] is missing"),
        Err(refusal) => return *refusal,
    };
    let Some(actions) = body.get("actions").and_then(Value::as_array) else {
        return validation_failed("[actions] is missing");
    };
    if actions.is_empty() {
        return validation_failed("Must specify at least one alias action");
    }
    if actions.len() > MAX_ALIAS_ACTIONS {
        return error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "a request may carry at most {MAX_ALIAS_ACTIONS} alias actions, and this one \
                 carries {}",
                actions.len()
            ),
        );
    }
    // Read and check every action BEFORE anything is written, so the whole request is
    // applied or none of it is.
    let mut planned: Vec<(bool, BTreeSet<String>, BTreeSet<String>, Value)> = Vec::new();
    let mut pairs = 0_usize;
    for action in actions {
        let Some((name, body)) = action.as_object().and_then(|members| members.iter().next())
        else {
            return validation_failed("[actions] must each name one alias action");
        };
        let adds = match name.as_str() {
            "add" => true,
            "remove" => false,
            other => {
                return error(
                    400,
                    ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                    &format!(
                        "[aliases] unknown alias action [{other}], expected [add] or [remove]"
                    ),
                )
            }
        };
        let indexes = named(body, INDEX_MEMBERS);
        if indexes.is_empty() {
            return validation_failed(&format!("[{}] is missing", INDEX_MEMBERS[0]));
        }
        let aliases = named(body, ALIAS_MEMBERS);
        if aliases.is_empty() {
            return validation_failed(&format!("[{}] is missing", ALIAS_MEMBERS[0]));
        }
        for index_name in &indexes {
            if !domain.indexes.contains_key(index_name) {
                return index_not_found(index_name);
            }
        }
        // What the action COSTS is its indexes times its aliases, and the names in each are
        // de-duplicated, so the pairs counted here are the ones the loop below performs.
        pairs = pairs.saturating_add(indexes.len().saturating_mul(aliases.len()));
        if pairs > MAX_ALIAS_PAIRS {
            return error(
                400,
                ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                &format!(
                    "a request may act on at most {MAX_ALIAS_PAIRS} index/alias pairs, and this \
                     one names {pairs}"
                ),
            );
        }
        planned.push((adds, indexes, aliases, action.clone()));
    }
    for (adds, indexes, aliases, action) in planned {
        let body = action
            .as_object()
            .and_then(|members| members.values().next())
            .cloned()
            .unwrap_or_else(|| json!({}));
        for index_name in &indexes {
            for alias in &aliases {
                let outcome = if adds {
                    add(domain, index_name, alias, &body)
                } else {
                    remove(domain, index_name, alias)
                };
                if let Err(refusal) = outcome {
                    return *refusal;
                }
            }
        }
    }
    ok(&json!({ "acknowledged": true }))
}

/// Point `alias` at `index_name`, carrying what the action declared.
fn add(
    domain: &mut Domain,
    index_name: &str,
    alias: &str,
    action: &Value,
) -> Result<(), Box<RtResponse>> {
    if alias.is_empty() {
        return Err(Box::new(validation_failed(&format!(
            "[{}] may not be empty string",
            ALIAS_MEMBERS[0]
        ))));
    }
    // "An alias cannot have the same name as an index": the two live in one name space,
    // which is what lets a read address either.
    if domain.indexes.contains_key(alias) {
        return Err(Box::new(error(
            400,
            INVALID_ALIAS_NAME_EXCEPTION,
            &format!("Invalid alias name [{alias}]: an index or data stream exists with the same name as the alias"),
        )));
    }
    let Some(index) = domain.indexes.get_mut(index_name) else {
        return Err(Box::new(index_not_found(index_name)));
    };
    if !index.aliases.contains_key(alias) && index.aliases.len() >= ALIAS_QUOTA {
        return Err(Box::new(error(
            400,
            ingest::ILLEGAL_ARGUMENT_EXCEPTION,
            &format!(
                "index [{index_name}] already carries the maximum of {ALIAS_QUOTA} aliases, so \
                 alias [{alias}] was not added"
            ),
        )));
    }
    // The action's own members are kept apart from the alias's name and its write flag:
    // what the alias API reports back is the filter and routing the action declared.
    let mut definition = Map::new();
    if let Some(members) = action.as_object() {
        for (member, value) in members {
            if ALIAS_MEMBERS.contains(&member.as_str())
                || INDEX_MEMBERS.contains(&member.as_str())
                || member == IS_WRITE_INDEX
            {
                continue;
            }
            definition.insert(member.clone(), value.clone());
        }
    }
    index.aliases.insert(
        alias.to_string(),
        IndexAlias {
            alias: alias.to_string(),
            is_write_index: action.get(IS_WRITE_INDEX).and_then(Value::as_bool),
            definition: Value::Object(definition),
        },
    );
    Ok(())
}

/// Stop `alias` pointing at `index_name`.
///
/// "the specified index or alias does not exist": removing an alias an index does not
/// carry is the engine's own missing-alias refusal rather than a success, so a client
/// rotating a write alias is told when the rotation it asked for did not happen.
fn remove(domain: &mut Domain, index_name: &str, alias: &str) -> Result<(), Box<RtResponse>> {
    let Some(index) = domain.indexes.get_mut(index_name) else {
        return Err(Box::new(index_not_found(index_name)));
    };
    if index.aliases.remove(alias).is_none() {
        return Err(Box::new(error(
            404,
            ALIASES_NOT_FOUND_EXCEPTION,
            &format!("aliases [{alias}] missing"),
        )));
    }
    Ok(())
}

/// The aliases a read reports: every index of the domain, or the ONE the path names, each
/// with the aliases it carries that the path's alias filter admits.
fn listing(domain: &Domain, index_name: Option<&str>, alias: Option<&str>) -> Value {
    let mut reported = Map::new();
    for index in domain.indexes.values() {
        if index_name.is_some_and(|named| named != index.index_name) {
            continue;
        }
        let mut aliases = Map::new();
        for held in index.aliases.values() {
            if alias.is_some_and(|named| named != held.alias) {
                continue;
            }
            aliases.insert(held.alias.clone(), reported_alias(held));
        }
        reported.insert(
            index.index_name.clone(),
            json!({ "aliases": Value::Object(aliases) }),
        );
    }
    Value::Object(reported)
}

/// The aliases of ONE index, as the index's own read reports them under `aliases`.
pub(super) fn reported(index: &state::Index) -> Value {
    let mut aliases = Map::new();
    for held in index.aliases.values() {
        aliases.insert(held.alias.clone(), reported_alias(held));
    }
    Value::Object(aliases)
}

/// One alias as a read reports it: what the action declared, with the write flag beside it
/// where the action named one.
fn reported_alias(alias: &IndexAlias) -> Value {
    let mut reported = alias
        .definition
        .as_object()
        .cloned()
        .unwrap_or_else(Map::new);
    if let Some(write_index) = alias.is_write_index {
        reported.insert(IS_WRITE_INDEX.to_string(), json!(write_index));
    }
    Value::Object(reported)
}

/// The aliases an index CREATE declares, from its `aliases` section — the same records an
/// alias action would have written.
pub(super) fn declared(section: Option<&Value>) -> BTreeMap<String, IndexAlias> {
    let mut declared = BTreeMap::new();
    let Some(Value::Object(members)) = section else {
        return declared;
    };
    for (alias, body) in members.iter().take(ALIAS_QUOTA) {
        if alias.is_empty() {
            continue;
        }
        let mut definition = body.as_object().cloned().unwrap_or_else(Map::new);
        let is_write_index = definition.remove(IS_WRITE_INDEX).and_then(|v| v.as_bool());
        declared.insert(
            alias.clone(),
            IndexAlias {
                alias: alias.clone(),
                is_write_index,
                definition: Value::Object(definition),
            },
        );
    }
    declared
}

/// The names an action gives under either spelling of a member — one name or a list of
/// them — as a SET, so a name an action repeats is acted on once however the list
/// interleaves it: what the action asks for is which indexes carry which aliases, and that
/// is a set.
fn named(action: &Value, members: [&str; 2]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for member in members {
        match action.get(member) {
            Some(Value::String(one)) => {
                names.insert(one.clone());
            }
            Some(Value::Array(listed)) => {
                names.extend(
                    listed
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        // The set is bounded by what one request may act on, so a list
                        // longer than that cannot be materialised either.
                        .take(MAX_ALIAS_PAIRS.saturating_add(1)),
                );
            }
            _ => {}
        }
    }
    names.retain(|name| !name.is_empty());
    names
}

/// The refusal a request whose body is not a well-formed action list earns: the engine's
/// own request-validation failure, which numbers the reasons it collected.
fn validation_failed(reason: &str) -> RtResponse {
    error(
        400,
        ingest::ILLEGAL_ARGUMENT_EXCEPTION,
        &format!("Validation Failed: 1: {reason};"),
    )
}

/// The same refusal for a per-index form that named no alias at all.
fn missing(member: &str) -> RtResponse {
    validation_failed(&format!("[{member}] is missing"))
}
