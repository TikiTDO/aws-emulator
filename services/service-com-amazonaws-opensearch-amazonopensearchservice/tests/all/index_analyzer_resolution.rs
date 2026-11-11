// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the analyzer a field mapping may name, over `CreateIndex`,
//! `UpdateIndex` and the `GetIndex` read that proves what each one persisted.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness and the per-operation request helpers
//! come from the sibling suites; this one adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a mapped field names its analyzer BY NAME, and a write of that mapping is served
//!   only where the name resolves — the search engine ships the analyzer, or the index's
//!   own analysis settings define one under that name — over a table differing in which
//!   mapping member carries the name, how deeply the field sits, which section defines
//!   the analyzer and whether the definition came with this write or an earlier one;
//! * a write naming an analyzer nothing resolves is refused with the modelled
//!   `ValidationException` (400) wording the engine's own sentence, and persists NOTHING:
//!   the index keeps the schema it had (or, for a create, is not there at all) and the
//!   next valid write is still served;
//! * every analyzer the published catalog holds resolves on an index that configures
//!   none, and a name just outside it does not;
//! * an analyzer is configured for the index whose settings define it and for no other,
//!   so one index's definition does not resolve another's mapping.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::BUILT_IN_ANALYZERS;

use super::control_plane_refusal_wording::message;
use super::data_source_registry::created_domain;
use super::domain_lifecycle::{error_code, Emulator, RECORDED_NOT_FOUND_STATUS};
use super::index_registry::{create as create_index, created_index, get as get_index, stored_names};
use super::index_schema_updates::{read_schema, update, updated_index};

/// The status `GetIndex` answers for a name that names no index of a domain the store
/// does hold — the status this service carries every missing-resource refusal on, the
/// one the sibling index suite pins.
const INDEX_NOT_FOUND_STATUS: u16 = RECORDED_NOT_FOUND_STATUS;

/// The domain every case in this suite drives.
const DOMAIN: &str = "analysis-domain";

/// The sentence a write naming an unresolvable analyzer is refused with: the engine's
/// own, with the analyzer the request named in it. Stated once here and asserted per
/// case, so no case pins a sentence of its own.
fn unconfigured_sentence(analyzer: &str) -> String {
    format!("analyzer [{analyzer}] has not been configured in mappings")
}

/// Whether `document` carries `text` as a string value anywhere in it — how an accepted
/// write is shown to have landed the analyzer the mapping named, wherever in the schema
/// that mapping sits.
fn holds_string(document: &Value, text: &str) -> bool {
    match document {
        Value::String(value) => value == text,
        Value::Array(elements) => elements.iter().any(|child| holds_string(child, text)),
        Value::Object(members) => members.values().any(|child| holds_string(child, text)),
        _ => false,
    }
}

/// A change this suite applies to prove a refused write left the index usable: it names
/// no analyzer at all, so nothing but the refusal under test can refuse it.
fn plain_change() -> Value {
    json!({ "mappings": { "properties": { "kept": { "type": "keyword" } } } })
}

/// One case about the analyzer an index's mappings name: the schema the index is created
/// with, the update applied to it, the analyzer name the case turns on, and whether
/// anything configures that name.
struct Row {
    case: &'static str,
    created: Value,
    update: Value,
    /// The analyzer the update's mappings name: the one nothing configures where the
    /// update is refused, else one the index resolves.
    analyzer: &'static str,
    unconfigured: bool,
}

/// The table every case about `UpdateIndex` runs over.
///
/// `IndexSchema` is an unconstrained document carrying "any changes to mappings,
/// settings, and semantic enrichment configuration"
/// (`smithy_query(query="operation",operation="UpdateIndex")`), so the rows differ along
/// what the rule generalises over: which mapping member names the analyzer, where the
/// named field sits (top level, an object field's property, a multi-field), which
/// section defines a custom analyzer and whether this write or the create defined it.
fn update_rows() -> Vec<Row> {
    vec![
        Row {
            case: "an analyzer the engine ships, on a top-level text field",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({
                "mappings": { "properties": { "title": { "analyzer": "english" } } }
            }),
            analyzer: "english",
            unconfigured: false,
        },
        Row {
            case: "an analyzer the engine ships, on a multi-field under fields",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({
                "mappings": {
                    "properties": {
                        "title": { "fields": { "cjk": { "type": "text", "analyzer": "cjk" } } }
                    }
                }
            }),
            analyzer: "cjk",
            unconfigured: false,
        },
        Row {
            case: "a query-time override naming an analyzer the engine ships",
            created: json!({ "mappings": { "properties": { "body": { "type": "text" } } } }),
            update: json!({
                "mappings": {
                    "properties": {
                        "body": { "search_analyzer": "fingerprint", "analyzer": "simple" }
                    }
                }
            }),
            analyzer: "fingerprint",
            unconfigured: false,
        },
        Row {
            case: "a custom analyzer this same write defines under settings.analysis",
            created: json!({ "mappings": { "properties": { "body": { "type": "text" } } } }),
            update: json!({
                "settings": {
                    "analysis": {
                        "analyzer": {
                            "house_analyzer": { "type": "custom", "tokenizer": "standard" }
                        }
                    }
                },
                "mappings": { "properties": { "body": { "analyzer": "house_analyzer" } } }
            }),
            analyzer: "house_analyzer",
            unconfigured: false,
        },
        Row {
            case: "a custom analyzer this same write defines under settings.index.analysis",
            created: json!({ "mappings": { "properties": { "body": { "type": "text" } } } }),
            update: json!({
                "settings": {
                    "index": {
                        "analysis": {
                            "analyzer": {
                                "nested_analyzer": { "type": "custom", "tokenizer": "keyword" }
                            }
                        }
                    }
                },
                "mappings": { "properties": { "body": { "analyzer": "nested_analyzer" } } }
            }),
            analyzer: "nested_analyzer",
            unconfigured: false,
        },
        Row {
            case: "a custom analyzer an EARLIER write defined on this index",
            created: json!({
                "settings": {
                    "analysis": {
                        "analyzer": {
                            "stored_analyzer": { "type": "custom", "tokenizer": "standard" }
                        }
                    }
                },
                "mappings": { "properties": { "body": { "type": "text" } } }
            }),
            update: json!({
                "mappings": {
                    "properties": {
                        "body": { "analyzer": "stored_analyzer" },
                        "note": { "type": "text", "search_quote_analyzer": "stored_analyzer" }
                    }
                }
            }),
            analyzer: "stored_analyzer",
            unconfigured: false,
        },
        Row {
            case: "a name nothing configures, on a top-level text field",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({
                "mappings": {
                    "properties": { "body": { "type": "text", "analyzer": "missing_analyzer" } }
                }
            }),
            analyzer: "missing_analyzer",
            unconfigured: true,
        },
        Row {
            case: "a name nothing configures, as the query-time analyzer",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({
                "mappings": {
                    "properties": {
                        "title": { "search_analyzer": "absent-at-search" }
                    }
                }
            }),
            analyzer: "absent-at-search",
            unconfigured: true,
        },
        Row {
            case: "a name nothing configures, as the quoted-phrase analyzer",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({
                "mappings": {
                    "properties": { "title": { "search_quote_analyzer": "no.such.analyzer" } }
                }
            }),
            analyzer: "no.such.analyzer",
            unconfigured: true,
        },
        Row {
            case: "a name nothing configures, on an object field's own property",
            created: json!({
                "mappings": { "properties": { "author": { "properties": {} } } }
            }),
            update: json!({
                "mappings": {
                    "properties": {
                        "author": {
                            "properties": {
                                "name": { "type": "text", "analyzer": "deep_missing" }
                            }
                        }
                    }
                }
            }),
            analyzer: "deep_missing",
            unconfigured: true,
        },
        Row {
            case: "a name nothing configures, on a multi-field under fields",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({
                "mappings": {
                    "properties": {
                        "title": {
                            "fields": { "folded": { "type": "text", "analyzer": "folded_x" } }
                        }
                    }
                }
            }),
            analyzer: "folded_x",
            unconfigured: true,
        },
        Row {
            case: "an analyzer DEFINED in the write, beside one nothing configures",
            created: json!({ "mappings": { "properties": { "title": { "type": "text" } } } }),
            update: json!({
                "settings": {
                    "analysis": {
                        "analyzer": { "defined_one": { "type": "custom", "tokenizer": "standard" } }
                    }
                },
                "mappings": {
                    "properties": {
                        "a": { "type": "text", "analyzer": "defined_one" },
                        "b": { "type": "text", "analyzer": "undefined_one" }
                    }
                }
            }),
            analyzer: "undefined_one",
            unconfigured: true,
        },
    ]
}

#[tokio::test]
async fn an_update_is_served_exactly_where_the_analyzer_its_mappings_name_resolves() {
    let emulator = Emulator::new();
    created_domain(&emulator, DOMAIN).await;

    for (at, row) in update_rows().iter().enumerate() {
        let index = format!("analyzer-row-{at}");
        let case = row.case;
        created_index(&emulator, DOMAIN, &index, &row.created, case).await;

        let analyzer = row.analyzer;
        match row.unconfigured {
            false => {
                updated_index(&emulator, DOMAIN, &index, &row.update, case).await;
                // The write landed: the analyzer the mapping named is in the schema the
                // modelled reader reports.
                let stored = read_schema(&emulator, DOMAIN, &index, case).await;
                assert!(
                    holds_string(&stored, analyzer),
                    "case {case:?}: expected the accepted mapping's analyzer {analyzer:?} in \
                     the stored schema, got {stored:?}"
                );
            }
            true => {
                let answer = update(&emulator, DOMAIN, &index, &row.update).await;
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with {:?}",
                    answer.status, answer.body
                );
                let code = error_code(&answer);
                assert_eq!(
                    code, "ValidationException",
                    "case {case:?}: expected ValidationException, got {code:?} with {:?}",
                    answer.body
                );
                let said = message(&answer);
                assert_eq!(
                    said,
                    unconfigured_sentence(analyzer),
                    "case {case:?}: expected the engine's sentence for an analyzer nothing \
                     configures, got {said:?}"
                );

                // The rejected mapping left NO trace: neither the analyzer name nor the
                // field that named it is in the index's schema.
                let stored = read_schema(&emulator, DOMAIN, &index, case).await;
                assert_eq!(
                    stored, row.created,
                    "case {case:?}: expected the index to keep the schema it was created \
                     with, got {stored:?}"
                );
                assert!(
                    !holds_string(&stored, analyzer),
                    "case {case:?}: expected no trace of {analyzer:?} in the stored schema, \
                     got {stored:?}"
                );

                // And the index is still usable: the next valid update is served.
                updated_index(&emulator, DOMAIN, &index, &plain_change(), case).await;
                let after = read_schema(&emulator, DOMAIN, &index, case).await;
                assert!(
                    holds_string(&after, "keyword"),
                    "case {case:?}: expected the following valid update to land, got {after:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn every_published_built_in_analyzer_resolves_and_a_name_outside_the_catalog_does_not() {
    let emulator = Emulator::new();
    created_domain(&emulator, DOMAIN).await;
    created_index(
        &emulator,
        DOMAIN,
        "catalog-index",
        &json!({ "mappings": { "properties": {} } }),
        "the catalog",
    )
    .await;

    // Driven over the WHOLE published catalog rather than the one name a caller is most
    // likely to write, so every entry of it is asserted and not just the cited one.
    for (at, analyzer) in BUILT_IN_ANALYZERS.iter().enumerate() {
        let field = format!("field_{at}");
        let schema = json!({
            "mappings": { "properties": { field: { "type": "text", "analyzer": analyzer } } }
        });
        updated_index(&emulator, DOMAIN, "catalog-index", &schema, analyzer).await;
    }

    let stored = read_schema(&emulator, DOMAIN, "catalog-index", "the catalog").await;
    for analyzer in BUILT_IN_ANALYZERS {
        assert!(
            holds_string(&stored, analyzer),
            "case {analyzer:?}: expected the accepted mapping in the stored schema, got \
             {stored:?}"
        );
    }

    // One name just outside the catalog: the shape of a built-in name with nothing
    // behind it.
    let outside = "standard_plus";
    assert!(
        !BUILT_IN_ANALYZERS.contains(&outside),
        "case {outside:?}: expected a name the catalog does not hold"
    );
    let answer = update(
        &emulator,
        DOMAIN,
        "catalog-index",
        &json!({
            "mappings": { "properties": { "outside": { "type": "text", "analyzer": outside } } }
        }),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case {outside:?}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    let said = message(&answer);
    assert_eq!(
        said,
        unconfigured_sentence(outside),
        "case {outside:?}: expected the engine's sentence, got {said:?}"
    );
}

/// One create case: the schema the create carries and the analyzer it names which
/// nothing configures, `None` where every name resolves.
struct CreateRow {
    case: &'static str,
    schema: Value,
    unconfigured: Option<&'static str>,
}

/// The table every case about `CreateIndex` runs over — the same rule as the update
/// table, over the write that MINTS the index: a create that names an analyzer nothing
/// configures must leave no index behind at all.
fn create_rows() -> Vec<CreateRow> {
    vec![
        CreateRow {
            case: "a create naming an analyzer the engine ships",
            schema: json!({
                "mappings": { "properties": { "title": { "type": "text", "analyzer": "standard" } } }
            }),
            unconfigured: None,
        },
        CreateRow {
            case: "a create defining the analyzer its own mappings name",
            schema: json!({
                "settings": {
                    "analysis": {
                        "analyzer": { "own_analyzer": { "type": "custom", "tokenizer": "standard" } }
                    }
                },
                "mappings": { "properties": { "title": { "analyzer": "own_analyzer" } } }
            }),
            unconfigured: None,
        },
        CreateRow {
            case: "a create naming an analyzer nothing configures",
            schema: json!({
                "mappings": { "properties": { "title": { "type": "text", "analyzer": "nope" } } }
            }),
            unconfigured: Some("nope"),
        },
        CreateRow {
            case: "a create naming one nothing configures on a multi-field",
            schema: json!({
                "mappings": {
                    "properties": {
                        "title": {
                            "type": "text",
                            "fields": { "raw": { "type": "text", "search_analyzer": "raw_x" } }
                        }
                    }
                }
            }),
            unconfigured: Some("raw_x"),
        },
        CreateRow {
            case: "a create whose analyzer is defined under the WRONG section",
            schema: json!({
                "settings": { "analyzer": { "sideways": { "type": "custom" } } },
                "mappings": { "properties": { "title": { "analyzer": "sideways" } } }
            }),
            unconfigured: Some("sideways"),
        },
    ]
}

#[tokio::test]
async fn a_create_naming_an_analyzer_nothing_configures_is_refused_and_persists_no_index() {
    let emulator = Emulator::new();
    created_domain(&emulator, DOMAIN).await;

    for (at, row) in create_rows().iter().enumerate() {
        let index = format!("create-row-{at}");
        let case = row.case;
        match row.unconfigured {
            None => {
                created_index(&emulator, DOMAIN, &index, &row.schema, case).await;
                let read = get_index(&emulator, DOMAIN, &index).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected the created index to read back, got {} with {:?}",
                    read.status, read.body
                );
                assert_eq!(
                    read.body["IndexSchema"], row.schema,
                    "case {case:?}: expected the schema it was created with, got {:?}",
                    read.body["IndexSchema"]
                );
            }
            Some(analyzer) => {
                let answer = create_index(
                    &emulator,
                    DOMAIN,
                    json!({ "IndexName": index, "IndexSchema": row.schema }),
                )
                .await;
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with {:?}",
                    answer.status, answer.body
                );
                let code = error_code(&answer);
                assert_eq!(
                    code, "ValidationException",
                    "case {case:?}: expected ValidationException, got {code:?} with {:?}",
                    answer.body
                );
                let said = message(&answer);
                assert_eq!(
                    said,
                    unconfigured_sentence(analyzer),
                    "case {case:?}: expected the engine's sentence for an analyzer nothing \
                     configures, got {said:?}"
                );

                // Nothing was minted: the store holds no such index and the modelled
                // reader answers its missing-resource refusal.
                let held = stored_names(&emulator, DOMAIN, case);
                assert!(
                    !held.contains(&index),
                    "case {case:?}: expected no index {index:?} to be stored, got {held:?}"
                );
                let read = get_index(&emulator, DOMAIN, &index).await;
                assert_eq!(
                    read.status,
                    INDEX_NOT_FOUND_STATUS,
                    "case {case:?}: expected the refused create to leave no index, got {} \
                     with {:?}",
                    read.status,
                    read.body
                );
                let code = error_code(&read);
                assert_eq!(
                    code, "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {code:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_analyzer_is_configured_for_the_index_whose_settings_define_it_and_for_no_other() {
    let emulator = Emulator::new();
    created_domain(&emulator, DOMAIN).await;
    created_domain(&emulator, "other-domain").await;

    let defining = json!({
        "settings": {
            "analysis": {
                "analyzer": { "house_analyzer": { "type": "custom", "tokenizer": "standard" } }
            }
        },
        "mappings": { "properties": { "title": { "type": "text" } } }
    });
    let plain = json!({ "mappings": { "properties": { "title": { "type": "text" } } } });
    created_index(&emulator, DOMAIN, "defining-index", &defining, "the definer").await;
    created_index(&emulator, DOMAIN, "plain-index", &plain, "a sibling index").await;
    created_index(&emulator, "other-domain", "plain-index", &plain, "another domain").await;

    let naming = json!({
        "mappings": { "properties": { "title": { "analyzer": "house_analyzer" } } }
    });

    // The index whose own settings define it resolves it.
    updated_index(&emulator, DOMAIN, "defining-index", &naming, "the definer").await;
    let stored = read_schema(&emulator, DOMAIN, "defining-index", "the definer").await;
    assert!(
        holds_string(&stored, "house_analyzer"),
        "case the definer: expected the analyzer in its own schema, got {stored:?}"
    );

    // No other index does — not a sibling on the same domain, and not the same index
    // name on another domain.
    for (domain, case) in [
        (DOMAIN, "a sibling index"),
        ("other-domain", "another domain"),
    ] {
        let answer = update(&emulator, domain, "plain-index", &naming).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        let said = message(&answer);
        assert_eq!(
            said,
            unconfigured_sentence("house_analyzer"),
            "case {case:?}: expected the engine's sentence, got {said:?}"
        );
        let stored = read_schema(&emulator, domain, "plain-index", case).await;
        assert_eq!(
            stored, plain,
            "case {case:?}: expected the index to keep the schema it was created with, got \
             {stored:?}"
        );
    }
}
