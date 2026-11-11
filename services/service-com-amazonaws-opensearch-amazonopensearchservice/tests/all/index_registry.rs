// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service index registry: `CreateIndex` and the
//! `GetIndex` read that proves what it persisted.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it, only the index
//! paths it drives.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a create persists the index under its name on the named domain and reports
//!   `CREATED`, and the schema reads back VERBATIM — over a table of schema documents
//!   differing in which sections they carry, in nesting depth, in JSON value type and
//!   in size, and of names differing in length and character class, with each
//!   domain's set holding only its own indexes;
//! * `IndexName` is accepted exactly where its modelled constraint allows it — each
//!   published length bound and one past it, plus one row per shape its pattern
//!   refuses — and a refused create persists nothing;
//! * an `IndexSchema` that is not a JSON object is refused, over a table of one row
//!   per other JSON value type;
//! * a create on a name the domain already holds is refused with the modelled
//!   already-exists error and leaves the stored schema exactly as it was;
//! * both operations refuse a domain the store does not hold, and `GetIndex` refuses
//!   a name that names no index of a domain it does hold, with the modelled
//!   `ResourceNotFoundException`.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::INDEX_STATUS_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::INDEX_STATUS_CREATED;

use super::data_source_registry::created_domain;
use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};

/// `CreateIndex`: `POST /opensearch/domain/{DomainName}/index`. A sibling suite drives
/// the same path for the creates it refuses, rather than growing a second copy of it.
pub(crate) async fn create(emulator: &Emulator, domain: &str, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{domain}/index"),
            Some(body),
        )
        .await
}

/// `GetIndex`: `GET /opensearch/domain/{DomainName}/index/{IndexName}`. The reader a
/// sibling suite proves its writes through, rather than growing a second copy of it.
pub(crate) async fn get(emulator: &Emulator, domain: &str, index: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{domain}/index/{index}"),
            None,
        )
        .await
}

/// `DeleteDomain`, so a case can reach a domain name the store no longer holds.
pub(crate) async fn delete_domain(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/{name}"), None)
        .await
}

/// The index names the store holds for `domain`, read the way a handler reaches the
/// store. Used where no implemented operation can report the set — a create refused
/// for a malformed name must persist NOTHING, and no listing operation exists.
pub(crate) fn stored_names(emulator: &Emulator, domain: &str, case: &str) -> Vec<String> {
    let store = emulator.store();
    let guard = store.lock();
    guard
        .domains
        .get(domain)
        .unwrap_or_else(|| panic!("case {case}: expected the store to hold domain {domain}"))
        .indexes
        .keys()
        .cloned()
        .collect()
}

/// A create that must be accepted, asserting the status it reports.
pub(crate) async fn created_index(
    emulator: &Emulator,
    domain: &str,
    index: &str,
    schema: &Value,
    case: &str,
) {
    let answer = create(
        emulator,
        domain,
        json!({ "IndexName": index, "IndexSchema": schema }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case CreateIndex({case}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let status = string_member(&answer.body, "Status", case);
    assert_eq!(
        status, INDEX_STATUS_CREATED,
        "case CreateIndex({case}): expected the status of a create, got {status:?}"
    );
    assert!(
        INDEX_STATUS_VALUES.contains(&status.as_str()),
        "case CreateIndex({case}): expected one of the modelled {INDEX_STATUS_VALUES:?}, got \
         {status:?}"
    );
}

/// One create to make: the index's name and the `IndexSchema` document it carries.
struct Row {
    name: String,
    schema: Value,
}

/// A schema with `fields` text fields, so the table holds a row whose document is far
/// larger than the hand-written ones.
pub(crate) fn wide_schema(fields: usize) -> Value {
    let properties: serde_json::Map<String, Value> = (0..fields)
        .map(|field| (format!("field_{field}"), json!({ "type": "text" })))
        .collect();
    json!({ "mappings": { "properties": properties } })
}

/// The create table: one row per way an accepted create differs from another — which
/// schema sections the document carries, how deeply it nests, which JSON value types
/// its leaves have and how large it is, and the name at its shortest, at its longest
/// and spanning the character classes its pattern allows.
///
/// `IndexName` is `@length(min: 1, max: 255)` and `IndexSchema` is an unconstrained
/// document
/// (`smithy_query(query="operation",operation="CreateIndex")`).
fn create_rows() -> Vec<Row> {
    vec![
        Row {
            // The documented shape: mappings naming the text fields to enrich.
            name: "logs-2026.01_v2".to_string(),
            schema: json!({
                "mappings": {
                    "properties": {
                        "message": { "type": "text", "semantic_enrichment": { "enabled": true } },
                        "level": { "type": "keyword" }
                    }
                }
            }),
        },
        Row {
            // Settings beside mappings: a second section in one document.
            name: "orders".to_string(),
            schema: json!({
                "settings": { "index": { "number_of_shards": 2, "number_of_replicas": 1 } },
                "mappings": { "properties": { "title": { "type": "text" } } }
            }),
        },
        Row {
            // A schema with no sections at all is still a schema object.
            name: "a".to_string(),
            schema: json!({}),
        },
        Row {
            // Every JSON value type a document leaf can have, and four levels of
            // nesting, so the round trip is not a string-only one.
            name: "mixed.types_1".to_string(),
            schema: json!({
                "string": "text",
                "unsigned": 1024,
                "negative": -7,
                "float": 1.5,
                "true": true,
                "false": false,
                "null": null,
                "array": [1, "two", { "three": 3 }, []],
                "nested": { "deep": { "deeper": { "leaf": "x" } } }
            }),
        },
        Row {
            // A name at the published maximum length.
            name: "i".repeat(255),
            schema: json!({ "mappings": { "properties": {} } }),
        },
        Row {
            // A name leading with a digit and one with a dot, both of which the
            // pattern allows: only `.` and `..` themselves are refused.
            name: "0metrics".to_string(),
            schema: wide_schema(50),
        },
        Row {
            name: ".hidden-index".to_string(),
            schema: json!({ "settings": { "index": { "knn": true } } }),
        },
    ]
}

#[tokio::test]
async fn every_created_index_is_read_back_with_the_schema_it_was_created_with() {
    let emulator = Emulator::new();
    created_domain(&emulator, "catalog-domain").await;
    created_domain(&emulator, "other-domain").await;

    let rows = create_rows();
    for row in &rows {
        let case = format!("{:?}", row.name);
        created_index(&emulator, "catalog-domain", &row.name, &row.schema, &case).await;

        // Read back through the modelled reader: the schema is reported VERBATIM,
        // whatever its sections, nesting, value types or size.
        let read = get(&emulator, "catalog-domain", &row.name).await;
        assert_eq!(
            read.status, 200,
            "case GetIndex({case}): expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["IndexSchema"], row.schema,
            "case GetIndex({case}): expected the schema as created, got {:?}",
            read.body["IndexSchema"]
        );
    }

    // The store holds exactly the created set, under the names the creates carried.
    let mut names = stored_names(&emulator, "catalog-domain", "whole table");
    names.sort();
    let mut expected: Vec<String> = rows.iter().map(|row| row.name.clone()).collect();
    expected.sort();
    assert_eq!(
        names, expected,
        "case CreateIndex(whole table): expected the stored set {expected:?}, got {names:?}"
    );

    // An index belongs to ONE domain: a second domain holds none of them, and a read
    // of one of their names against it is refused.
    let other = stored_names(&emulator, "other-domain", "other domain");
    assert!(
        other.is_empty(),
        "case CreateIndex(other domain): expected no indexes on a domain nobody created one on, \
         got {other:?}"
    );
    for row in &rows {
        let case = format!("other-domain/{:?}", row.name);
        let read = get(&emulator, "other-domain", &row.name).await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case GetIndex({case}): expected {RECORDED_NOT_FOUND_STATUS} for another domain's \
             index, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case GetIndex({case}): expected ResourceNotFoundException, got {:?}",
            read.body
        );
    }
}

#[tokio::test]
async fn an_index_name_is_accepted_exactly_where_the_modelled_constraint_allows_it() {
    let emulator = Emulator::new();
    created_domain(&emulator, "name-rules").await;

    // `IndexName` is `@length(min: 1, max: 255)` with pattern
    // `^(?!\.\.?$)[^_ ,:"+/*\\|?#><A-Z-][^ ,:"+/*\\|?#><A-Z]*$`
    // (`smithy_query(query="shape",shape="IndexName")`): each published length bound
    // and one past it, plus one row per shape the pattern refuses — the two names its
    // lookahead excludes, a leading character its first class excludes, and one row
    // per character class forbidden anywhere in the name.
    let shortest = "i".to_string();
    let longest = "i".repeat(255);
    let past_longest = "i".repeat(256);
    let rows: Vec<(&str, bool)> = vec![
        (shortest.as_str(), true),
        (longest.as_str(), true),
        ("logs.2026-01-01_v2", true),
        // One past each published length bound.
        ("", false),
        (past_longest.as_str(), false),
        // The two names the pattern's lookahead refuses outright, and the dotted
        // names it does NOT refuse.
        (".", false),
        ("..", false),
        (".leading-dot", true),
        ("...", true),
        // The leading class refuses an underscore and a hyphen in first position,
        // and allows both anywhere after it.
        ("_leading", false),
        ("-leading", false),
        ("trailing_", true),
        ("trailing-", true),
        // An upper-case letter is refused wherever it appears.
        ("Leading-upper", false),
        ("trailinG", false),
        // One row per punctuation class both character classes forbid.
        ("has space", false),
        ("has,comma", false),
        ("has:colon", false),
        ("has\"quote", false),
        ("has+plus", false),
        ("has/slash", false),
        ("has*star", false),
        ("has\\backslash", false),
        ("has|pipe", false),
        ("has?question", false),
        ("has#hash", false),
        ("has>greater", false),
        ("has<less", false),
    ];

    let mut accepted: Vec<String> = Vec::new();
    for (name, allowed) in rows {
        let case = format!("CreateIndex({name:?})");
        let answer = create(
            &emulator,
            "name-rules",
            json!({ "IndexName": name, "IndexSchema": { "mappings": {} } }),
        )
        .await;
        if allowed {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 inside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            accepted.push(name.to_string());
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400 outside the constraint, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {:?}",
                answer.body
            );
        }

        // Whatever the row, the stored set is exactly the rows accepted so far: a
        // refused create persists nothing.
        let mut names = stored_names(&emulator, "name-rules", &case);
        names.sort();
        let mut expected = accepted.clone();
        expected.sort();
        assert_eq!(
            names, expected,
            "case {case}: expected the stored set {expected:?}, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_schema_that_is_not_a_json_object_is_refused_and_persists_nothing() {
    let emulator = Emulator::new();
    created_domain(&emulator, "schema-rules").await;

    // `IndexSchema` is the document "defining index mappings, settings, and semantic
    // enrichment configuration" — a set of named sections. One row per other JSON
    // value type a document can carry, plus the request that omits it altogether.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("a string", Some(json!("mappings"))),
        ("an integer", Some(json!(7))),
        ("a float", Some(json!(1.5))),
        ("a boolean", Some(json!(true))),
        ("an array of sections", Some(json!([{ "mappings": {} }]))),
        ("an empty array", Some(json!([]))),
        ("null", Some(Value::Null)),
        ("omitted", None),
    ];

    for (case, schema) in rows {
        let mut body = json!({ "IndexName": "schema-row" });
        if let Some(schema) = schema {
            body["IndexSchema"] = schema;
        }
        let answer = create(&emulator, "schema-rules", body).await;
        assert_eq!(
            answer.status, 400,
            "case CreateIndex({case}): expected 400 for a schema that is not an object, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case CreateIndex({case}): expected ValidationException, got {:?}",
            answer.body
        );
        let names = stored_names(&emulator, "schema-rules", case);
        assert!(
            names.is_empty(),
            "case CreateIndex({case}): a refused create must persist nothing, got {names:?}"
        );
    }

    // The same name, with an object schema, is accepted: nothing above poisoned it.
    created_index(
        &emulator,
        "schema-rules",
        "schema-row",
        &json!({ "mappings": {} }),
        "an object",
    )
    .await;
}

#[tokio::test]
async fn a_create_on_a_name_the_domain_holds_is_refused_and_leaves_the_schema_as_it_was() {
    let emulator = Emulator::new();
    created_domain(&emulator, "already-exists").await;

    // Every row of the create table, so the rule holds over the whole class rather
    // than on one name: the second create is refused and the stored schema is the
    // FIRST one, not the schema the refused call carried.
    for row in create_rows() {
        let case = format!("{:?}", row.name);
        created_index(
            &emulator,
            "already-exists",
            &row.name,
            &row.schema,
            &case,
        )
        .await;

        let second = create(
            &emulator,
            "already-exists",
            json!({ "IndexName": row.name, "IndexSchema": { "settings": { "overwritten": true } } }),
        )
        .await;
        assert_eq!(
            second.status, 409,
            "case CreateIndex({case}, again): expected 409 for a name the domain holds, got {} \
             with {:?}",
            second.status, second.body
        );
        assert_eq!(
            error_code(&second),
            "ResourceAlreadyExistsException",
            "case CreateIndex({case}, again): expected ResourceAlreadyExistsException, got {:?}",
            second.body
        );

        let read = get(&emulator, "already-exists", &row.name).await;
        assert_eq!(
            read.body["IndexSchema"], row.schema,
            "case GetIndex({case}, after the refused create): expected the schema the accepted \
             create carried, got {:?}",
            read.body["IndexSchema"]
        );
    }
}

#[tokio::test]
async fn both_operations_refuse_a_domain_or_an_index_the_store_does_not_hold() {
    let emulator = Emulator::new();
    created_domain(&emulator, "held-domain").await;
    created_domain(&emulator, "released-domain").await;
    let released = delete_domain(&emulator, "released-domain").await;
    assert_eq!(
        released.status, 200,
        "case DeleteDomain(released-domain): expected 200, got {} with {:?}",
        released.status, released.body
    );

    // A domain name inside its own constraint that the store holds no domain for:
    // never created, and created then released.
    for domain in ["never-created", "released-domain"] {
        let create_answer = create(
            &emulator,
            domain,
            json!({ "IndexName": "any-index", "IndexSchema": { "mappings": {} } }),
        )
        .await;
        assert_eq!(
            create_answer.status,
            RECORDED_NOT_FOUND_STATUS,
            "case CreateIndex({domain:?}): expected {RECORDED_NOT_FOUND_STATUS} for a domain the \
             store does not hold, got {} with {:?}",
            create_answer.status,
            create_answer.body
        );
        assert_eq!(
            error_code(&create_answer),
            "ResourceNotFoundException",
            "case CreateIndex({domain:?}): expected ResourceNotFoundException, got {:?}",
            create_answer.body
        );

        let read = get(&emulator, domain, "any-index").await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case GetIndex({domain:?}): expected {RECORDED_NOT_FOUND_STATUS} for a domain the \
             store does not hold, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case GetIndex({domain:?}): expected ResourceNotFoundException, got {:?}",
            read.body
        );
    }

    // A domain the store DOES hold, and a name that names no index of it — before any
    // create, and beside one that exists.
    created_index(
        &emulator,
        "held-domain",
        "present-index",
        &json!({ "mappings": {} }),
        "present",
    )
    .await;
    for index in ["absent-index", "present-index.v2"] {
        let read = get(&emulator, "held-domain", index).await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case GetIndex(held-domain/{index:?}): expected {RECORDED_NOT_FOUND_STATUS} for a \
             name that names no index, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case GetIndex(held-domain/{index:?}): expected ResourceNotFoundException, got {:?}",
            read.body
        );
    }
}
