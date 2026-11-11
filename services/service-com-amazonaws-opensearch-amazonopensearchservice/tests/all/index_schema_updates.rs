// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateIndex`, read back through `GetIndex`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the create/read helpers from
//! [`super::index_registry`]; this suite adds only the update path.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * an update is a CHANGE to the stored schema: every section, property and member
//!   the request names is written and everything the request does not name stays —
//!   over a table differing in which section is touched, in nesting depth, in JSON
//!   value type, in how many members one call writes and in whether the member
//!   existed before — and it answers `UPDATED`;
//! * a field's type is settled by the write that first maps it, so an update naming
//!   a different type for a field the index already maps is refused with the
//!   modelled `ValidationException` (400) and changes nothing — over a table of
//!   field positions (top level, an object field's property, a multi-field) and
//!   type pairs, against rows the rule does NOT refuse (the same type restated, a
//!   type for a field the index does not map yet, a `type` member outside the
//!   mappings);
//! * so of N concurrent updates mapping ONE field with N different types, exactly
//!   one lands and the index ends carrying that one's type — never a blend;
//! * because an update merges, an update that would carry one index's schema past
//!   the emulator's own per-index bound is refused and changes nothing;
//! * an update refuses a domain or an index the store does not hold, and a
//!   malformed request, persisting nothing.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::INDEX_STATUS_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    self, INDEX_SCHEMA_MAX_DEPTH, INDEX_SCHEMA_QUOTA, INDEX_STATUS_UPDATED,
};

use super::data_source_registry::created_domain;
use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};
use super::index_registry::{create as create_index_call, created_index, get as get_index, stored_names};

/// `UpdateIndex`: `PUT /opensearch/domain/{DomainName}/index/{IndexName}`. A sibling
/// suite drives the same path, rather than growing a second copy of it.
pub(crate) async fn update(emulator: &Emulator, domain: &str, index: &str, schema: &Value) -> Answer {
    emulator
        .call(
            "PUT",
            &format!("{PREFIX}/opensearch/domain/{domain}/index/{index}"),
            Some(json!({ "IndexSchema": schema })),
        )
        .await
}

/// An update that must be accepted, asserting the status it reports.
pub(crate) async fn updated_index(
    emulator: &Emulator,
    domain: &str,
    index: &str,
    schema: &Value,
    case: &str,
) {
    let answer = update(emulator, domain, index, schema).await;
    assert_eq!(
        answer.status, 200,
        "case UpdateIndex({case}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let status = string_member(&answer.body, "Status", case);
    assert_eq!(
        status, INDEX_STATUS_UPDATED,
        "case UpdateIndex({case}): expected the status of an update, got {status:?}"
    );
    assert!(
        INDEX_STATUS_VALUES.contains(&status.as_str()),
        "case UpdateIndex({case}): expected one of the modelled {INDEX_STATUS_VALUES:?}, got \
         {status:?}"
    );
}

/// The schema `GetIndex` reports for `index`, with the case named on failure.
pub(crate) async fn read_schema(emulator: &Emulator, domain: &str, index: &str, case: &str) -> Value {
    let read = get_index(emulator, domain, index).await;
    assert_eq!(
        read.status, 200,
        "case GetIndex({case}): expected 200, got {} with {:?}",
        read.status, read.body
    );
    read.body["IndexSchema"].clone()
}

/// One merge case: the schema the index is created with, the update to apply and the
/// schema the index must then carry.
struct MergeRow {
    case: &'static str,
    created: Value,
    update: Value,
    merged: Value,
}

/// A mapping of `fields` text properties, so a row can write far more members in one
/// call than the hand-written rows do.
fn wide_properties(first: usize, fields: usize) -> Value {
    let properties: serde_json::Map<String, Value> = (first..first + fields)
        .map(|field| (format!("field_{field}"), json!({ "type": "text" })))
        .collect();
    json!({ "mappings": { "properties": properties } })
}

/// Every member of `wide_properties(0, 3)` and `wide_properties(3, 25)` in one
/// document: what merging the second onto the first must leave.
fn wide_merged() -> Value {
    let properties: serde_json::Map<String, Value> = (0..28)
        .map(|field| (format!("field_{field}"), json!({ "type": "text" })))
        .collect();
    json!({ "mappings": { "properties": properties } })
}

/// The merge table: one row per way an accepted update differs from another — which
/// section it touches, whether the member it names existed before, how deeply it
/// nests, which JSON value type it writes, and how many members one call writes.
///
/// `IndexSchema` is an unconstrained document carrying "any changes to mappings,
/// settings, and semantic enrichment configuration"
/// (`smithy_query(query="operation",operation="UpdateIndex")`), so the rows span
/// those sections rather than a validated set of section names.
fn merge_rows() -> Vec<MergeRow> {
    vec![
        MergeRow {
            case: "a property the index does not map yet, beside the one it does",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({ "mappings": { "properties": { "summary": { "type": "text" } } } }),
            merged: json!({
                "mappings": {
                    "properties": {
                        "title": { "type": "text" },
                        "summary": { "type": "text" }
                    }
                }
            }),
        },
        MergeRow {
            case: "a member added to a property already mapped, restating its type",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({
                "mappings": {
                    "properties": { "title": { "type": "text", "analyzer": "english" } }
                }
            }),
            merged: json!({
                "mappings": {
                    "properties": { "title": { "type": "text", "analyzer": "english" } }
                }
            }),
        },
        MergeRow {
            case: "semantic enrichment turned on for a field already mapped",
            created: json!({
                "mappings": {
                    "properties": {
                        "body": { "type": "text", "semantic_enrichment": { "enabled": false } }
                    }
                }
            }),
            update: json!({
                "mappings": {
                    "properties": { "body": { "semantic_enrichment": { "enabled": true } } }
                }
            }),
            merged: json!({
                "mappings": {
                    "properties": {
                        "body": { "type": "text", "semantic_enrichment": { "enabled": true } }
                    }
                }
            }),
        },
        MergeRow {
            case: "a section the stored schema does not carry",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({ "settings": { "index": { "number_of_replicas": 2 } } }),
            merged: json!({
                "mappings": { "properties": { "title": { "type": "text" } } },
                "settings": { "index": { "number_of_replicas": 2 } }
            }),
        },
        MergeRow {
            case: "one setting rewritten, its siblings left alone",
            created: json!({
                "settings": { "index": { "number_of_replicas": 1, "knn": true } }
            }),
            update: json!({ "settings": { "index": { "number_of_replicas": 4 } } }),
            merged: json!({
                "settings": { "index": { "number_of_replicas": 4, "knn": true } }
            }),
        },
        MergeRow {
            case: "a list is one value, so it is replaced rather than appended to",
            created: json!({
                "settings": { "analysis": { "filter": ["lowercase", "stop"], "char": [] } }
            }),
            update: json!({ "settings": { "analysis": { "filter": ["asciifolding"] } } }),
            merged: json!({
                "settings": { "analysis": { "filter": ["asciifolding"], "char": [] } }
            }),
        },
        MergeRow {
            case: "a property of an object field, beside the object's other property",
            created: json!({
                "mappings": {
                    "properties": {
                        "author": { "properties": { "name": { "type": "text" } } }
                    }
                }
            }),
            update: json!({
                "mappings": {
                    "properties": {
                        "author": { "properties": { "email": { "type": "keyword" } } }
                    }
                }
            }),
            merged: json!({
                "mappings": {
                    "properties": {
                        "author": {
                            "properties": {
                                "name": { "type": "text" },
                                "email": { "type": "keyword" }
                            }
                        }
                    }
                }
            }),
        },
        MergeRow {
            case: "a multi-field added under a field already mapped",
            created: json!({ "mappings": { "properties": { "city": { "type": "text" } } } }),
            update: json!({
                "mappings": {
                    "properties": { "city": { "fields": { "raw": { "type": "keyword" } } } }
                }
            }),
            merged: json!({
                "mappings": {
                    "properties": {
                        "city": { "type": "text", "fields": { "raw": { "type": "keyword" } } }
                    }
                }
            }),
        },
        MergeRow {
            case: "every JSON value type a member can be written as, five levels deep",
            created: json!({ "mappings": { "_meta": { "owner": "search-team" } } }),
            update: json!({
                "mappings": {
                    "_meta": {
                        "owner": null,
                        "revision": 7,
                        "ratio": 1.5,
                        "retired": false,
                        "tags": ["a", { "b": 2 }],
                        "deep": { "deeper": { "deepest": { "leaf": "x" } } }
                    }
                }
            }),
            merged: json!({
                "mappings": {
                    "_meta": {
                        "owner": null,
                        "revision": 7,
                        "ratio": 1.5,
                        "retired": false,
                        "tags": ["a", { "b": 2 }],
                        "deep": { "deeper": { "deepest": { "leaf": "x" } } }
                    }
                }
            }),
        },
        MergeRow {
            case: "twenty-five properties written by one call",
            created: wide_properties(0, 3),
            update: wide_properties(3, 25),
            merged: wide_merged(),
        },
        MergeRow {
            case: "an update that names no member changes nothing",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({}),
            merged: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
        },
        MergeRow {
            case: "a section that names no member changes nothing under it",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({ "mappings": {} }),
            merged: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
        },
        MergeRow {
            case: "an index created with no sections at all",
            created: json!({}),
            update: json!({
                "mappings": { "properties": { "title": { "type": "text" } } },
                "settings": { "index": { "number_of_shards": 3 } }
            }),
            merged: json!({
                "mappings": { "properties": { "title": { "type": "text" } } },
                "settings": { "index": { "number_of_shards": 3 } }
            }),
        },
        MergeRow {
            case: "a setting that is a value where a set of members stood",
            created: json!({ "settings": { "index": { "knn": { "enabled": true } } } }),
            update: json!({ "settings": { "index": { "knn": false } } }),
            merged: json!({ "settings": { "index": { "knn": false } } }),
        },
    ]
}

#[tokio::test]
async fn every_update_writes_what_it_names_and_leaves_the_rest_of_the_schema_alone() {
    let emulator = Emulator::new();
    created_domain(&emulator, "schema-updates").await;

    // One index per row, plus one index nobody updates: an update touches the index
    // it names and no other.
    created_index(
        &emulator,
        "schema-updates",
        "untouched",
        &json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
        "untouched",
    )
    .await;

    for (number, row) in merge_rows().into_iter().enumerate() {
        let index = format!("row-{number}");
        created_index(&emulator, "schema-updates", &index, &row.created, row.case).await;
        updated_index(&emulator, "schema-updates", &index, &row.update, row.case).await;

        let read = read_schema(&emulator, "schema-updates", &index, row.case).await;
        assert_eq!(
            read, row.merged,
            "case {}: expected the stored schema to become {:?}, got {read:?}",
            row.case, row.merged
        );

        // A second update lands on top of the first: the schema an update leaves is
        // the schema the next one changes, and it writes a section of its own, so
        // everything the row merged must still be there beside it.
        updated_index(
            &emulator,
            "schema-updates",
            &index,
            &json!({ "aliases": { "current": "v1" } }),
            row.case,
        )
        .await;
        let again = read_schema(&emulator, "schema-updates", &index, row.case).await;
        assert_eq!(
            again["aliases"]["current"],
            json!("v1"),
            "case {}: expected the second update to land too, got {again:?}",
            row.case
        );
        for (section, value) in row.merged.as_object().expect("a schema is an object") {
            assert_eq!(
                &again[section], value,
                "case {}: expected {section} to survive the second update, got {again:?}",
                row.case
            );
        }
    }

    let untouched = read_schema(&emulator, "schema-updates", "untouched", "untouched").await;
    assert_eq!(
        untouched,
        json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
        "case untouched: an update must change only the index it names, got {untouched:?}"
    );
}

/// One case for the settled-type rule: the schema the index is created with, the
/// update to attempt, and what must come of it — the field the refusal names with
/// both types, or the schema an accepted update leaves.
struct TypeRow {
    case: &'static str,
    created: Value,
    update: Value,
    outcome: Outcome,
}

/// What the settled-type rule makes of one row.
enum Outcome {
    /// The field the refusal names, the type it is mapped with and the type the
    /// update asked for.
    Refused(&'static str, &'static str, &'static str),
    /// The schema the index must carry once the update has landed.
    Merged(Value),
}

/// The settled-type table. The rule is about the type STRING differing from the one
/// the field is already mapped with, not about a published set of types, so the rows
/// use OpenSearch field type names only for realism; what each row varies is WHERE
/// the field sits in the mapping document and WHICH pair of types meet.
fn type_rows() -> Vec<TypeRow> {
    vec![
        TypeRow {
            case: "a top-level field re-typed from text to keyword",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({ "mappings": { "properties": { "title": { "type": "keyword" } } } }),
            outcome: Outcome::Refused("title", "text", "keyword"),
        },
        TypeRow {
            case: "a numeric field re-typed from integer to long",
            created: json!({ "mappings": { "properties": { "views": { "type": "integer" } } } }),
            update: json!({ "mappings": { "properties": { "views": { "type": "long" } } } }),
            outcome: Outcome::Refused("views", "integer", "long"),
        },
        TypeRow {
            case: "a keyword field re-typed to a number",
            created: json!({ "mappings": { "properties": { "sku": { "type": "keyword" } } } }),
            update: json!({ "mappings": { "properties": { "sku": { "type": "integer" } } } }),
            outcome: Outcome::Refused("sku", "keyword", "integer"),
        },
        TypeRow {
            case: "a property of an object field re-typed",
            created: json!({
                "mappings": {
                    "properties": {
                        "author": { "properties": { "name": { "type": "text" } } }
                    }
                }
            }),
            update: json!({
                "mappings": {
                    "properties": {
                        "author": { "properties": { "name": { "type": "keyword" } } }
                    }
                }
            }),
            outcome: Outcome::Refused("author.name", "text", "keyword"),
        },
        TypeRow {
            case: "a multi-field of a field re-typed",
            created: json!({
                "mappings": {
                    "properties": {
                        "city": { "type": "text", "fields": { "raw": { "type": "keyword" } } }
                    }
                }
            }),
            update: json!({
                "mappings": {
                    "properties": { "city": { "fields": { "raw": { "type": "text" } } } }
                }
            }),
            outcome: Outcome::Refused("city.raw", "keyword", "text"),
        },
        TypeRow {
            case: "one re-typed field among changes that would otherwise land",
            created: json!({
                "mappings": {
                    "properties": {
                        "title": { "type": "text" },
                        "views": { "type": "integer" }
                    }
                }
            }),
            update: json!({
                "mappings": {
                    "properties": {
                        "summary": { "type": "text" },
                        "views": { "type": "keyword" }
                    }
                },
                "settings": { "index": { "number_of_replicas": 2 } }
            }),
            outcome: Outcome::Refused("views", "integer", "keyword"),
        },
        TypeRow {
            case: "the same type restated is no change of type",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({
                "mappings": {
                    "properties": { "title": { "type": "text", "index": false } }
                }
            }),
            outcome: Outcome::Merged(json!({
                "mappings": {
                    "properties": { "title": { "type": "text", "index": false } }
                }
            })),
        },
        TypeRow {
            case: "a type for a field the index does not map yet",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({ "mappings": { "properties": { "views": { "type": "integer" } } } }),
            outcome: Outcome::Merged(json!({
                "mappings": {
                    "properties": {
                        "title": { "type": "text" },
                        "views": { "type": "integer" }
                    }
                }
            })),
        },
        TypeRow {
            case: "a type named for a field of an object the index does not map yet",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({
                "mappings": {
                    "properties": {
                        "author": { "properties": { "name": { "type": "keyword" } } }
                    }
                }
            }),
            outcome: Outcome::Merged(json!({
                "mappings": {
                    "properties": {
                        "title": { "type": "text" },
                        "author": { "properties": { "name": { "type": "keyword" } } }
                    }
                }
            })),
        },
        TypeRow {
            case: "a type member outside the mappings is not a field's type",
            created: json!({ "settings": { "store": { "type": "fs" } } }),
            update: json!({ "settings": { "store": { "type": "niofs" } } }),
            outcome: Outcome::Merged(json!({ "settings": { "store": { "type": "niofs" } } })),
        },
        TypeRow {
            case: "an index with no mappings at all has no settled type",
            created: json!({}),
            update: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            outcome: Outcome::Merged(
                json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            ),
        },
        TypeRow {
            // The rule is about the type the index already carries, whatever stands
            // where the type name belongs: a value there is settled the same way.
            case: "a type that is not a name is settled all the same",
            created: json!({ "mappings": { "properties": { "views": { "type": 7 } } } }),
            update: json!({ "mappings": { "properties": { "views": { "type": "integer" } } } }),
            outcome: Outcome::Refused("views", "7", "integer"),
        },
        TypeRow {
            case: "the same value restated where a type name belongs is no change",
            created: json!({ "mappings": { "properties": { "views": { "type": 7 } } } }),
            update: json!({
                "mappings": { "properties": { "views": { "type": 7, "index": false } } }
            }),
            outcome: Outcome::Merged(json!({
                "mappings": { "properties": { "views": { "type": 7, "index": false } } }
            })),
        },
    ]
}

#[tokio::test]
async fn an_update_that_would_re_type_a_mapped_field_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    created_domain(&emulator, "settled-types").await;

    for (number, row) in type_rows().into_iter().enumerate() {
        let index = format!("row-{number}");
        created_index(&emulator, "settled-types", &index, &row.created, row.case).await;

        match row.outcome {
            Outcome::Refused(field, from, to) => {
                let answer = update(&emulator, "settled-types", &index, &row.update).await;
                assert_eq!(
                    answer.status, 400,
                    "case {}: expected 400 for a change of a settled field type, got {} with {:?}",
                    row.case, answer.status, answer.body
                );
                assert_eq!(
                    error_code(&answer),
                    "ValidationException",
                    "case {}: expected ValidationException, got {:?}",
                    row.case,
                    answer.body
                );
                let message = string_member(&answer.body, "message", row.case);
                for part in [field, from, to] {
                    assert!(
                        message.contains(&format!("[{part}]")),
                        "case {}: expected the refusal to name [{part}], got {message:?}",
                        row.case
                    );
                }

                // A refused update changes NOTHING, not even the members of it that
                // carried no conflict.
                let read = read_schema(&emulator, "settled-types", &index, row.case).await;
                assert_eq!(
                    read, row.created,
                    "case {}: expected the schema as created, got {read:?}",
                    row.case
                );
            }
            Outcome::Merged(merged) => {
                updated_index(&emulator, "settled-types", &index, &row.update, row.case).await;
                let read = read_schema(&emulator, "settled-types", &index, row.case).await;
                assert_eq!(
                    read, merged,
                    "case {}: expected the accepted update to leave {merged:?}, got {read:?}",
                    row.case
                );
            }
        }
    }
}

/// How many concurrent updates each row of the race table sends, and the types they
/// map one field with — one per racer, all different, so every racer but the first
/// to arrive asks for a type the field is by then already mapped with.
const RACES: [(&str, &[&str]); 3] = [
    ("two racers", &["integer", "keyword"]),
    ("three racers", &["text", "long", "boolean"]),
    (
        "five racers",
        &["text", "keyword", "integer", "float", "date"],
    ),
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_mapping_one_field_leave_exactly_one_winner() {
    let emulator = Arc::new(Emulator::new());
    created_domain(&emulator, "raced-schema").await;

    for (case, types) in RACES {
        let index = format!("raced-{}", types.len());
        // The field the racers map is one the index does NOT map yet, so no racer's
        // type is settled before the race starts.
        created_index(
            &emulator,
            "raced-schema",
            &index,
            &json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            case,
        )
        .await;

        let mut racers = Vec::new();
        for field_type in types {
            let emulator = Arc::clone(&emulator);
            let index = index.clone();
            let field_type = (*field_type).to_string();
            racers.push(tokio::spawn(async move {
                let schema =
                    json!({ "mappings": { "properties": { "rank": { "type": field_type } } } });
                let answer = update(&emulator, "raced-schema", &index, &schema).await;
                (field_type, answer)
            }));
        }

        let mut winners: Vec<String> = Vec::new();
        for racer in racers {
            let (field_type, answer) = racer.await.expect("the racing update task ran");
            if answer.status == 200 {
                let status = string_member(&answer.body, "Status", case);
                assert_eq!(
                    status, INDEX_STATUS_UPDATED,
                    "case {case} ({field_type}): expected the status of an update, got {status:?}"
                );
                winners.push(field_type);
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {case} ({field_type}): a racer either lands or is refused for the settled \
                 type, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case} ({field_type}): expected ValidationException, got {:?}",
                answer.body
            );
            let message = string_member(&answer.body, "message", case);
            assert!(
                message.contains("[rank]"),
                "case {case} ({field_type}): expected the refusal to name the raced field, got \
                 {message:?}"
            );
        }
        assert_eq!(
            winners.len(),
            1,
            "case {case}: expected exactly one racer to win, got {winners:?}"
        );

        // The index carries the winner's type — one racer's, never a blend of two —
        // and the mapping it had before the race.
        let read = read_schema(&emulator, "raced-schema", &index, case).await;
        assert_eq!(
            read,
            json!({
                "mappings": {
                    "properties": {
                        "title": { "type": "text" },
                        "rank": { "type": winners[0] }
                    }
                }
            }),
            "case {case}: expected the winner's mapping beside the one already there, got {read:?}"
        );
    }
}

#[tokio::test]
async fn an_update_that_would_grow_one_schema_past_the_emulators_own_bound_is_refused() {
    let emulator = Emulator::new();
    created_domain(&emulator, "schema-bound").await;

    // An update merges onto the stored document, so what the bound governs is the
    // MERGED schema: a document inside the bound is stored, and the update that
    // would carry the same index past it is refused and changes nothing. The bound
    // is the EMULATOR's own cap on how far one index's schema may grow (the model
    // publishes none), so the sizes come from the constant rather than from a
    // documented quota.
    //
    // `wide_properties(first, fields)` holds `3 + 2 * fields` values — the document,
    // its `mappings` and its `properties` set, then one set and one `type` member
    // per field — so these two sizes sit either side of it.
    let inside_fields = INDEX_SCHEMA_QUOTA / 4;
    let past_fields = INDEX_SCHEMA_QUOTA / 2;
    assert!(
        3 + 2 * inside_fields <= INDEX_SCHEMA_QUOTA,
        "case inside the bound: {inside_fields} fields must stay inside {INDEX_SCHEMA_QUOTA}"
    );
    assert!(
        3 + 2 * (inside_fields + past_fields) > INDEX_SCHEMA_QUOTA,
        "case past the bound: {inside_fields} plus {past_fields} fields must pass \
         {INDEX_SCHEMA_QUOTA}"
    );

    let inside = wide_properties(0, inside_fields);
    created_index(&emulator, "schema-bound", "bounded", &inside, "inside the bound").await;
    updated_index(
        &emulator,
        "schema-bound",
        "bounded",
        &json!({ "settings": { "index": { "number_of_replicas": 1 } } }),
        "inside the bound",
    )
    .await;

    let past = wide_properties(inside_fields, past_fields);
    let answer = update(&emulator, "schema-bound", "bounded", &past).await;
    assert_eq!(
        answer.status, 400,
        "case past the bound: expected 400 for a merged schema past the bound, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case past the bound: expected ValidationException, got {:?}",
        answer.body
    );
    let message = string_member(&answer.body, "message", "past the bound");
    assert!(
        message.contains(&INDEX_SCHEMA_QUOTA.to_string()),
        "case past the bound: expected the refusal to report the bound, got {message:?}"
    );

    // The refused update changed nothing: the schema is the one the accepted calls
    // left, and the index still takes an update that stays inside the bound.
    let read = read_schema(&emulator, "schema-bound", "bounded", "past the bound").await;
    assert_eq!(
        read["mappings"], inside["mappings"],
        "case past the bound: expected the mappings the create wrote, got {read:?}"
    );

    // The same bound holds for the create that would PERSIST such a document, so no
    // index starts out past the bound every later update is held to: the create is
    // refused and mints nothing.
    let past_on_create = wide_properties(0, INDEX_SCHEMA_QUOTA);
    let refused = create_index_call(
        &emulator,
        "schema-bound",
        json!({ "IndexName": "over-bound", "IndexSchema": past_on_create }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case past the bound on create: expected 400 for a schema past the bound, got {} with \
         {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case past the bound on create: expected ValidationException, got {:?}",
        refused.body
    );
    let created_message = string_member(&refused.body, "message", "past the bound on create");
    assert!(
        created_message.contains(&INDEX_SCHEMA_QUOTA.to_string()),
        "case past the bound on create: expected the refusal to report the bound, got \
         {created_message:?}"
    );
    let held = stored_names(&emulator, "schema-bound", "past the bound on create");
    assert!(
        !held.contains(&"over-bound".to_string()),
        "case past the bound on create: a refused create must mint nothing, got {held:?}"
    );

    updated_index(
        &emulator,
        "schema-bound",
        "bounded",
        &json!({ "settings": { "index": { "number_of_replicas": 2 } } }),
        "after the refusal",
    )
    .await;
}

/// A schema document nesting exactly `depth` levels: a leaf value under `depth - 1`
/// named sets, which is what `state::index_schema_depth` measures as `depth`.
fn nested_schema(depth: usize) -> Value {
    let mut node = json!("text");
    for _ in 1..depth {
        node = json!({ "nested": node });
    }
    node
}

/// RULE: an `IndexSchema` nesting deeper than the emulator's own depth bound is refused
/// with the modelled `ValidationException` (400) and changes nothing — by the create
/// that would persist it and by the update that would merge it alike — while a document
/// AT the bound is accepted. The walks that merge a schema and compare its mapped types
/// address a member by its path, so this is what keeps the cost of accepting a document
/// tied to the bytes the request carried. The rows are the published bound, one past it
/// and far past it.
#[tokio::test]
async fn a_schema_nesting_past_the_emulators_own_depth_bound_is_refused() {
    let emulator = Emulator::new();
    created_domain(&emulator, "schema-depth").await;
    let created = json!({ "mappings": { "properties": { "title": { "type": "text" } } } });
    created_index(&emulator, "schema-depth", "deep", &created, "at the start").await;

    // (the case, the depth the document nests to, whether it must be accepted, whether
    // the refusal is the handler's own — past some depth the request boundary's own
    // deserializer refuses the document first, with its own 400, before any handler
    // sees it)
    let rows: Vec<(&str, usize, bool, bool)> = vec![
        ("at the bound", INDEX_SCHEMA_MAX_DEPTH, true, false),
        ("one past the bound", INDEX_SCHEMA_MAX_DEPTH + 1, false, true),
        ("well past the bound", INDEX_SCHEMA_MAX_DEPTH + 20, false, true),
        ("deeper than any request may carry", 1_000, false, false),
    ];

    for (case, depth, accepted, handler_refusal) in rows {
        let schema = nested_schema(depth);
        let minted = format!("deep-{depth}");
        // Each row is driven through BOTH operations that carry an `IndexSchema`: the
        // update that merges it onto a stored schema and the create that persists it.
        let answers = [
            ("UpdateIndex", update(&emulator, "schema-depth", "deep", &schema).await),
            (
                "CreateIndex",
                create_index_call(
                    &emulator,
                    "schema-depth",
                    json!({ "IndexName": minted.clone(), "IndexSchema": schema.clone() }),
                )
                .await,
            ),
        ];
        for (operation, answer) in &answers {
            if accepted {
                assert_eq!(
                    answer.status, 200,
                    "case {operation}({case}): a document at the bound must be accepted, got {} \
                     with {:?}",
                    answer.status, answer.body
                );
                continue;
            }
            assert_eq!(
                answer.status, 400,
                "case {operation}({case}): expected 400 for a document past the depth bound, got \
                 {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(answer),
                "ValidationException",
                "case {operation}({case}): expected ValidationException, got {:?}",
                answer.body
            );
            if !handler_refusal {
                continue;
            }
            let message = string_member(&answer.body, "message", case);
            assert!(
                message.contains(&INDEX_SCHEMA_MAX_DEPTH.to_string())
                    && message.contains(&depth.to_string()),
                "case {operation}({case}): expected the refusal to report the depth {depth} and \
                 the bound {INDEX_SCHEMA_MAX_DEPTH}, got {message:?}"
            );
        }

        // A refused create persisted no index; an accepted one is the index it named.
        let held = stored_names(&emulator, "schema-depth", case);
        assert_eq!(
            held.contains(&minted),
            accepted,
            "case {case}: expected the store to hold {minted:?} only where the create was \
             accepted, got {held:?}"
        );
    }

    // Every refusal above left the stored schema's own mappings exactly as they were,
    // and the index still takes a well-formed update.
    let read = read_schema(&emulator, "schema-depth", "deep", "after the refusals").await;
    assert_eq!(
        read["mappings"], created["mappings"],
        "case after the refusals: expected the mappings the create wrote, got {read:?}"
    );
    updated_index(
        &emulator,
        "schema-depth",
        "deep",
        &json!({ "mappings": { "properties": { "summary": { "type": "text" } } } }),
        "after the refusals",
    )
    .await;
}

/// RULE: converting a schema between the JSON a REST request carries and the document
/// this service stores it as preserves the value — a value converted to a document and
/// back is the value it started as, with its nesting intact — at every nesting a caller
/// can hand over, including past the depth a stored schema may reach.
///
/// The conversion walks hold their state on the heap rather than one stack frame per
/// level, because the nesting is the CALLER's: the rows therefore run from a flat value
/// to one nesting several times the bound this service enforces on a stored schema and
/// past the depth the request boundary's own deserializer admits, over both kinds of
/// container and the leaf value types a schema carries.
///
/// The deepest row is capped by what the TEST can do, not by what the conversion can: a
/// `serde_json::Value` is compared and dropped by walks of its own that recurse per
/// level, so a row deep enough to overrun a recursive conversion would overrun this
/// test's own comparison first — and a stack overflow aborts the whole test binary
/// rather than failing one case. What the rows pin is therefore that the walk preserves
/// the value and its nesting; costing no stack is a property of the implementation,
/// which holds no per-level frame at all.
#[test]
fn the_schema_conversions_round_trip_at_any_nesting() {
    // (the case, the depth the value nests to)
    let rows: Vec<(&str, usize)> = vec![
        ("a flat value", 1),
        ("inside the stored bound", INDEX_SCHEMA_MAX_DEPTH / 2),
        ("at the stored bound", INDEX_SCHEMA_MAX_DEPTH),
        ("past what any request may carry", INDEX_SCHEMA_MAX_DEPTH * 3),
    ];

    for (case, depth) in rows {
        // Both kinds of container at every level, so the walk is exercised over lists as
        // well as named sets, beside the leaf value types a schema carries.
        let mut value = json!({ "type": "text", "boost": 1.5, "indexed": true, "copy_to": null });
        for _ in 1..depth {
            value = json!({ "properties": [value] });
        }

        let document = state::document_from_json(&value);
        let back = state::json_from_document(&document);
        assert_eq!(
            back, value,
            "case {case:?} (depth {depth}): expected the conversion to round-trip, got {back:?}"
        );
        assert_eq!(
            state::index_schema_depth(&document),
            // Two levels per step of the construction above — the named set and the list
            // inside it — and two for the leaf set and its members.
            depth * 2,
            "case {case:?}: expected the document to nest as deeply as the value it was \
             converted from, got {:?}",
            state::index_schema_depth(&document)
        );
    }
}

#[tokio::test]
async fn an_update_refuses_what_the_store_does_not_hold_and_a_malformed_request() {
    let emulator = Emulator::new();
    created_domain(&emulator, "update-refusals").await;
    created_index(
        &emulator,
        "update-refusals",
        "present-index",
        &json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
        "present",
    )
    .await;
    let stored = json!({ "mappings": { "properties": { "title": { "type": "text" } } } });
    let change = json!({ "mappings": { "properties": { "summary": { "type": "text" } } } });

    // A domain name inside its own constraint that the store holds no domain for.
    for domain in ["never-created", "also-never-created"] {
        let answer = update(&emulator, domain, "present-index", &change).await;
        assert_eq!(
            answer.status,
            RECORDED_NOT_FOUND_STATUS,
            "case UpdateIndex({domain:?}): expected {RECORDED_NOT_FOUND_STATUS} for a domain the \
             store does not hold, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case UpdateIndex({domain:?}): expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    // A domain the store DOES hold, and a name that names no index of it: an update
    // mints nothing, so it is refused rather than creating the index.
    for index in ["absent-index", "present-index.v2"] {
        let answer = update(&emulator, "update-refusals", index, &change).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case UpdateIndex({index:?}): expected {RECORDED_NOT_FOUND_STATUS} for a name that \
             names no index, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case UpdateIndex({index:?}): expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        let read = get_index(&emulator, "update-refusals", index).await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case GetIndex({index:?}): a refused update must mint nothing, got {} with {:?}",
            read.status, read.body
        );
    }

    // A name no index could have, and a schema that is not a set of sections: both
    // malformed requests, both refused with the modelled 400. (The rows are the
    // refused shapes a URI can carry; `index_registry` drives the whole pattern.)
    let malformed_names = ["_leading", "-leading", "Upper", "."];
    for name in malformed_names {
        let answer = update(&emulator, "update-refusals", name, &change).await;
        assert_eq!(
            answer.status, 400,
            "case UpdateIndex({name:?}): expected 400 for a name no index could have, got {} with \
             {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case UpdateIndex({name:?}): expected ValidationException, got {:?}",
            answer.body
        );
    }
    for (case, schema) in [
        ("a string", json!("mappings")),
        ("an integer", json!(7)),
        ("a float", json!(1.5)),
        ("a boolean", json!(true)),
        ("an array of sections", json!([{ "mappings": {} }])),
        ("null", Value::Null),
    ] {
        let answer = update(&emulator, "update-refusals", "present-index", &schema).await;
        assert_eq!(
            answer.status, 400,
            "case UpdateIndex({case}): expected 400 for a schema that is not a set of sections, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case UpdateIndex({case}): expected ValidationException, got {:?}",
            answer.body
        );
    }

    // Every refusal above left the stored schema exactly as the create wrote it.
    let read = read_schema(&emulator, "update-refusals", "present-index", "after refusals").await;
    assert_eq!(
        read, stored,
        "case UpdateIndex(after refusals): expected the schema as created, got {read:?}"
    );

    // And the same index still takes an update that is well formed.
    updated_index(
        &emulator,
        "update-refusals",
        "present-index",
        &change,
        "well formed",
    )
    .await;
}
