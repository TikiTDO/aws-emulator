// SPDX-License-Identifier: CC0-1.0

//! The data plane's INDEX SETTINGS API: `GET`/`PUT /{index}/_settings`, and the same two
//! addressed to the whole cluster as `/_settings`.
//!
//! "Index settings" are the per-index configuration an index carries beside its mappings:
//! how many shards and replica copies it has, how often it refreshes, which analysis chain
//! its text goes through, and the slow-log thresholds past which an operation on it is
//! recorded. A GET reports the settings each index the path names holds, and a PUT writes
//! the ones its body names — the DYNAMIC ones, since a setting that can only be chosen when
//! the index is created is refused on an index that exists ([`STATIC_INDEX_SETTINGS`]).
//!
//! Both halves read and write the ONE copy of an index's settings this emulator holds: the
//! `IndexSchema` the index record carries ([`crate::state::index_settings`]), which is also
//! what the create wrote, what `GetIndex` reports and what the cluster health, a refresh and
//! the analysis chain are computed from. So a setting a client writes here is the setting
//! every one of those then reads, and nothing is stored twice.

use ::serde_json::{json, Map, Value};
use aws_smithy_runtime_api::http::Response as RtResponse;

use crate::state::{self, fgac, ingest, Domain};

use super::{
    error, flatten_settings, guarded, ok, reported_settings, resolved_indexes, stable_uuid,
    wrong_method, Request, WrittenSetting, PARSE_EXCEPTION,
};

/// The index settings that can be chosen only when the index is CREATED — the engine's
/// static index settings, the ones its documentation marks as settable at index creation
/// and not through the update-settings API. Everything else an index carries is dynamic,
/// so a PUT naming one of these is refused and a PUT naming anything else is applied.
///
/// Each name is the setting's own name below `index.`, which is the form a request's names
/// are canonicalised to ([`crate::state::index_setting_name`]).
const STATIC_INDEX_SETTINGS: [&str; 9] = [
    "number_of_shards",
    "number_of_routing_shards",
    "routing_partition_size",
    "codec",
    "shard.check_on_startup",
    "sort.field",
    "sort.order",
    "store.type",
    // The analysis chain is built when the index is created, so the whole section — every
    // analyzer, tokenizer and filter under it — may only change on a closed index.
    "analysis",
];

/// The two members the cluster fills in itself for every index, which a settings report
/// carries beside the settings the index was written with: the name the index was created
/// under, and the id the cluster gave it.
const PROVIDED_NAME: &str = "provided_name";

/// The second of them ([`PROVIDED_NAME`]).
const UUID: &str = "uuid";

/// One setting's value as a report carries it: the TEXT it is held as.
///
/// "Settings are returned as strings": the engine keeps a setting as the text it was
/// written with however a request spelled it, so a number and a boolean are reported as
/// their own spelling and a list as the list of its texts.
fn as_text(value: Value) -> Value {
    match value {
        Value::String(_) | Value::Null | Value::Object(_) => value,
        Value::Array(elements) => Value::Array(elements.into_iter().map(as_text).collect()),
        other => json!(other.to_string()),
    }
}

/// `GET`/`PUT /{index}/_settings`, and the same addressed to the whole cluster.
///
/// `named` is the index patterns the path named, where it named any: a request that names
/// none is about every index the cluster holds, which is what `/_settings` addresses.
///
/// `filter` is the setting-name filter a READ may narrow the report with
/// (`GET /{index}/_settings/{filter}`): the comma-separated names, each of which may carry
/// the `*` and `?` wildcards, of the settings to report.
pub(super) fn index_settings_request(
    domain: &mut Domain,
    request: &Request<'_>,
    named: Option<&str>,
    filter: Option<&str>,
) -> RtResponse {
    match request.method {
        "GET" | "HEAD" => guarded(domain, request, fgac::ACTION_GET_INDEX_SETTINGS, |domain| {
            reported(domain, request, named, filter)
        }),
        "PUT" => guarded(
            domain,
            request,
            fgac::ACTION_UPDATE_INDEX_SETTINGS,
            |domain| updated(domain, request, named),
        ),
        _ => wrong_method(request, "GET, HEAD, PUT"),
    }
}

/// The settings each index the request names holds, as the engine reports them: one entry
/// per index, each carrying its settings under the `index` section they are named below.
fn reported(
    domain: &Domain,
    request: &Request<'_>,
    named: Option<&str>,
    filter: Option<&str>,
) -> RtResponse {
    let indexes = match resolved_indexes(domain, named) {
        Ok(indexes) => indexes,
        Err(refusal) => return *refusal,
    };
    let mut answer = Map::new();
    for name in indexes {
        let Some(index) = domain.indexes.get(name) else {
            continue;
        };
        answer.insert(
            name.to_string(),
            json!({
                state::SCHEMA_SETTINGS: {
                    state::SETTINGS_INDEX_SECTION:
                        settings_view(domain, index, request, filter),
                }
            }),
        );
    }
    ok(&Value::Object(answer))
}

/// One index's settings as a report carries them: the settings it holds, plus the two
/// members the cluster fills in itself, in the EXPANDED sections their dotted names name —
/// or flat for a request asking for `flat_settings`, which is the one reporting rule the
/// cluster's own settings are reported by ([`reported_settings`]).
///
/// `filter` narrows the report to the names it admits. A setting is named either bare or
/// with the `index.` prefix its full name carries, so a filter is matched against both
/// spellings — which is what lets `index.number_of_*` select the shard counts.
pub(super) fn settings_view(
    domain: &Domain,
    index: &state::Index,
    request: &Request<'_>,
    filter: Option<&str>,
) -> Value {
    let held = state::index_settings(&index.schema);
    let mut flattened: Vec<WrittenSetting> = Vec::new();
    // The only refusal this flattening states is for a section nesting deeper than a
    // setting name may, and what is stored was bounded when it was written — so a section
    // that somehow reached that depth is reported as it is held rather than not at all.
    if flatten_settings(&held, &mut String::new(), &mut flattened).is_err() {
        return Value::Object(held);
    }
    let mut settings: Vec<(&str, Value)> = flattened
        .iter()
        .filter_map(|(name, value)| value.clone().map(|value| (name.as_str(), as_text(value))))
        .collect();
    settings.push((PROVIDED_NAME, json!(index.index_name)));
    settings.push((
        UUID,
        json!(stable_uuid(&format!("{}/{}", domain.arn, index.index_name))),
    ));
    if let Some(filter) = filter {
        let patterns: Vec<&str> = filter.split(',').filter(|name| !name.is_empty()).collect();
        settings.retain(|(name, _)| {
            patterns.iter().any(|pattern| {
                let matched = |value: &str| {
                    ::service_common::iam_authorization::wildcard_match(pattern, value, true)
                };
                matched(name)
                    || matched(&format!("{}.{name}", state::SETTINGS_INDEX_SECTION))
            })
        });
    }
    reported_settings(settings, request)
}

/// Apply the settings update the request's body asks for, to every index the path names.
///
/// The body names its settings "in a flattened or expanded format", and either spelling
/// names the same setting ([`flatten_settings`]), with or without the `index.` prefix every
/// index setting's full name carries ([`crate::state::index_setting_name`]). A setting
/// written `null` is taken out, which is how a client resets one to the cluster's default,
/// and a setting the body does not name is left exactly as it was: an update is of the
/// settings it names.
///
/// Read and checked WHOLE before anything is stored, over every index the path names, so a
/// refused update leaves every one of them carrying the settings it had.
fn updated(domain: &mut Domain, request: &Request<'_>, named: Option<&str>) -> RtResponse {
    let indexes: Vec<String> = match resolved_indexes(domain, named) {
        Ok(indexes) => indexes.into_iter().map(str::to_string).collect(),
        Err(refusal) => return *refusal,
    };
    let body = match request.json() {
        Ok(Some(Value::Object(members))) => members,
        Ok(_) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: expected the body to be a JSON object naming the settings to \
                 update",
            )
        }
        Err(refusal) => return *refusal,
    };
    let mut written: Vec<WrittenSetting> = Vec::new();
    if let Err(refusal) = flatten_settings(&body, &mut String::new(), &mut written) {
        return *refusal;
    }
    let updates: Vec<WrittenSetting> = written
        .into_iter()
        .map(|(name, value)| (state::index_setting_name(&name).to_string(), value))
        .collect();
    if let Some(refusal) = non_dynamic_refusal(domain, &indexes, &updates) {
        return refusal;
    }
    // The merged schema of every named index, built and bounded before one of them is
    // written: the bounds are the ones that hold for a schema however it was stored
    // ([`crate::state::INDEX_SCHEMA_MAX_DEPTH`], [`crate::state::INDEX_SCHEMA_QUOTA`]), so
    // an update cannot grow this process's state past what a create may.
    let mut merged: Vec<(String, ::aws_smithy_types::Document)> = Vec::new();
    for name in &indexes {
        let Some(index) = domain.indexes.get(name) else {
            continue;
        };
        let schema = state::with_index_settings(&index.schema, &updates);
        let depth = state::index_schema_depth(&schema);
        if depth > state::INDEX_SCHEMA_MAX_DEPTH {
            return error(
                400,
                ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                &format!("index schema nests {depth} levels, past the maximum this cluster takes"),
            );
        }
        let values = state::index_schema_values(&schema);
        if values > state::INDEX_SCHEMA_QUOTA {
            return error(
                400,
                ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                &format!("index schema holds {values} values, past the maximum this cluster takes"),
            );
        }
        merged.push((name.clone(), schema));
    }
    // The indexes whose schema this update really wrote, which is what the change is
    // recorded for: an index the resolution named and the write no longer found changed
    // nothing, so nothing is stated about it.
    let mut written: Vec<String> = Vec::new();
    for (name, schema) in merged {
        if let Some(index) = domain.indexes.get_mut(&name) {
            index.schema = schema;
            written.push(name);
        }
    }
    // An index's settings are part of the cluster's own state, so the change is recorded
    // the way the cluster's application log states one: per index written, naming the
    // settings the update wrote.
    let names: Vec<String> = updates
        .iter()
        .map(|(name, _)| format!("{}.{name}", state::SETTINGS_INDEX_SECTION))
        .collect();
    if !names.is_empty() {
        let change = state::log_publishing::IndexChange::SettingsUpdated { settings: names };
        let served: Vec<state::log_publishing::Served<'_>> = written
            .iter()
            .map(|index| state::log_publishing::Served::Applied {
                index: index.as_str(),
                change: change.clone(),
            })
            .collect();
        request.record_served(domain, &served);
    }
    ok(&json!({ "acknowledged": true }))
}

/// The refusal an update naming a setting the index cannot be given after it exists carries
/// — the engine's own answer for a static setting on an open index, naming the settings it
/// refused and the indexes the request was about.
///
/// `None` when every setting the update names is a dynamic one, which is the whole surface
/// apart from [`STATIC_INDEX_SETTINGS`].
fn non_dynamic_refusal(
    domain: &Domain,
    indexes: &[String],
    updates: &[WrittenSetting],
) -> Option<RtResponse> {
    let refused: Vec<String> = updates
        .iter()
        .map(|(name, _)| name)
        .filter(|name| is_static(name))
        .map(|name| format!("{}.{name}", state::SETTINGS_INDEX_SECTION))
        .collect();
    if refused.is_empty() {
        return None;
    }
    let open: Vec<String> = indexes
        .iter()
        .map(|name| format!("{name}/{}", stable_uuid(&format!("{}/{name}", domain.arn))))
        .collect();
    Some(error(
        400,
        ingest::ILLEGAL_ARGUMENT_EXCEPTION,
        &format!(
            "Can't update non dynamic settings [[{}]] for open indices [[{}]]",
            refused.join(", "),
            open.join(", ")
        ),
    ))
}

/// Whether `name` names a setting only a create may choose: one of
/// [`STATIC_INDEX_SETTINGS`], or a setting NESTED under one of them — every analyzer,
/// tokenizer and filter under `analysis` is part of the section the index was built with,
/// so each is as settled as the section itself.
fn is_static(name: &str) -> bool {
    STATIC_INDEX_SETTINGS.iter().any(|settled| {
        name == *settled
            || (name.starts_with(settled) && name.as_bytes().get(settled.len()) == Some(&b'.'))
    })
}
