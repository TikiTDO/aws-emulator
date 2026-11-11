// SPDX-License-Identifier: CC0-1.0

//! The data plane's INDEX MAPPING API: `GET`/`PUT /{index}/_mapping`, and the same two
//! addressed to the whole cluster as `/_mapping`.
//!
//! An index's mappings are the field types its documents are judged by — the ones a write
//! is accepted or refused against ([`crate::state::ingest`]) and the ones a `match` resolves
//! its analysis chain from. A GET reports the mappings each index the path names holds, and
//! a PUT merges the ones its body names onto them.
//!
//! One rule decides a write: "you can't change the mapping of an existing field". A field's
//! type is settled by the write that first mapped it, so an update may ADD a field (or a
//! multi-field under one) and may not re-type one — refused with the sentence
//! [`crate::state::mapping_type_immutable`] words for it, which is the same sentence
//! `UpdateIndex` answers, because it is one rule and not two.
//!
//! Both halves read and write the ONE copy of an index's mappings this emulator holds: the
//! `IndexSchema` the index record carries, which is also what the create wrote and what
//! `GetIndex` and the index's own read report.

use aws_smithy_runtime_api::http::Response as RtResponse;
use serde_json::{json, Map, Value};

use crate::state::{self, fgac, ingest, Domain};

use super::{error, guarded, ok, resolved_indexes, wrong_method, Request, PARSE_EXCEPTION};

/// `GET`/`PUT /{index}/_mapping`, and the same addressed to the whole cluster.
///
/// `named` is the index patterns the path named, where it named any: a request that names
/// none is about every index the cluster holds, which is what `/_mapping` addresses.
pub(super) fn index_mapping_request(
    domain: &mut Domain,
    request: &Request<'_>,
    named: Option<&str>,
) -> RtResponse {
    match request.method {
        "GET" | "HEAD" => guarded(domain, request, fgac::ACTION_GET_MAPPING, |domain| {
            reported(domain, named)
        }),
        "PUT" | "POST" => guarded(domain, request, fgac::ACTION_PUT_MAPPING, |domain| {
            updated(domain, request, named)
        }),
        _ => wrong_method(request, "GET, HEAD, PUT, POST"),
    }
}

/// The mappings each index the request names holds, as the engine reports them: one entry
/// per index, each carrying its mappings under the section they are named below.
fn reported(domain: &Domain, named: Option<&str>) -> RtResponse {
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
            json!({ state::SCHEMA_MAPPINGS: mappings_view(index) }),
        );
    }
    ok(&Value::Object(answer))
}

/// One index's mappings as a read reports them: the section it holds, and the empty object
/// for an index whose create named none.
pub(super) fn mappings_view(index: &state::Index) -> Value {
    state::json_from_document(&index.schema)
        .get(state::SCHEMA_MAPPINGS)
        .cloned()
        .unwrap_or_else(|| json!({}))
}

/// Merge the mappings the request's body carries onto every index the path names.
///
/// Read and checked WHOLE before anything is stored, over every index the path names, so a
/// refused update leaves every one of them carrying the mappings it had.
fn updated(domain: &mut Domain, request: &Request<'_>, named: Option<&str>) -> RtResponse {
    let indexes: Vec<String> = match resolved_indexes(domain, named) {
        Ok(indexes) => indexes.into_iter().map(str::to_string).collect(),
        Err(refusal) => return *refusal,
    };
    let body = match request.json() {
        Ok(Some(Value::Object(members))) => Value::Object(members),
        Ok(_) => {
            return error(
                400,
                PARSE_EXCEPTION,
                "failed to parse: expected the body to be a JSON object carrying the mappings to \
                 add",
            )
        }
        Err(refusal) => return *refusal,
    };
    if indexes.is_empty() {
        return error(400, ingest::ILLEGAL_ARGUMENT_EXCEPTION, "index is missing");
    }
    // The body IS the mappings (`{"properties": …}`), which is how the mapping API takes
    // them; a body that wraps them in the section name names the same thing.
    let mappings = body
        .get(state::SCHEMA_MAPPINGS)
        .cloned()
        .unwrap_or_else(|| body.clone());
    let update = state::document_from_json(&json!({ state::SCHEMA_MAPPINGS: mappings }));
    // The merged schema of every named index, built and bounded before one of them is
    // written: the bounds are the ones that hold for a schema however it was stored, so an
    // update cannot grow this process's state past what a create may.
    let mut merged: Vec<(String, ::aws_smithy_types::Document)> = Vec::new();
    for name in &indexes {
        let Some(index) = domain.indexes.get(name) else {
            continue;
        };
        if let Some(conflict) = state::mapping_type_conflict(&index.schema, &update) {
            return error(
                400,
                ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                &state::mapping_type_immutable(&conflict)
                    .message
                    .unwrap_or_default(),
            );
        }
        let schema = state::merged_index_schema(&index.schema, &update);
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
    for (name, schema) in merged {
        if let Some(index) = domain.indexes.get_mut(&name) {
            index.schema = schema;
        }
    }
    ok(&json!({ "acknowledged": true }))
}
