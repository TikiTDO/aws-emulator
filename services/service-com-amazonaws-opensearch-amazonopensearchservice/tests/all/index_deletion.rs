// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteIndex`, proved through the `CreateIndex` write and the
//! `GetIndex` read beside it.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the index paths from [`super::index_registry`];
//! this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a delete removes exactly the named index of the named domain, reports `DELETED`,
//!   and leaves every other index of that domain and of every other domain untouched
//!   — over a table of names differing in length and character class and of schemas
//!   differing in sections, nesting and size, and over every deletion ORDER of a
//!   three-index domain;
//! * the name a delete freed can be created again, with a different schema, and reads
//!   back as the fresh create left it;
//! * a delete is refused where the resource is not there to remove — a malformed
//!   `IndexName`, a domain the store does not hold, a name that names no index of a
//!   domain it does hold, and the second delete of one index — and a refused delete
//!   removes nothing.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::INDEX_STATUS_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::INDEX_STATUS_DELETED;

use super::data_source_registry::created_domain;
use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};
use super::index_registry::{created_index, delete_domain, get, stored_names, wide_schema};

/// `DeleteIndex`: `DELETE /opensearch/domain/{DomainName}/index/{IndexName}`
/// (`smithy_query(query="operation",operation="DeleteIndex")`).
async fn delete(emulator: &Emulator, domain: &str, index: &str) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/{domain}/index/{index}"),
            None,
        )
        .await
}

/// A delete that must be accepted, asserting the status it reports. A sibling suite
/// drives the same path, rather than growing a second copy of it.
pub(crate) async fn deleted_index(emulator: &Emulator, domain: &str, index: &str, case: &str) {
    let answer = delete(emulator, domain, index).await;
    assert_eq!(
        answer.status, 200,
        "case DeleteIndex({case}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let status = string_member(&answer.body, "Status", case);
    assert_eq!(
        status, INDEX_STATUS_DELETED,
        "case DeleteIndex({case}): expected the status of a delete, got {status:?}"
    );
    assert!(
        INDEX_STATUS_VALUES.contains(&status.as_str()),
        "case DeleteIndex({case}): expected one of the modelled {INDEX_STATUS_VALUES:?}, got \
         {status:?}"
    );
}

/// One index to create and then delete: its name and the `IndexSchema` it carries.
struct Row {
    name: &'static str,
    schema: Value,
}

/// The delete table: one row per way one index differs from another — the name at its
/// shortest, spanning the character classes its pattern allows, leading with a digit
/// and leading with a dot; the schema with no sections, with mappings only, with two
/// sections, and far larger than either.
///
/// `IndexName` is `@length(min: 1, max: 255)` with pattern
/// `^(?!\.\.?$)[^_ ,:"+/*\\|?#><A-Z-][^ ,:"+/*\\|?#><A-Z]*$`
/// (`smithy_query(query="shape",shape="IndexName")`).
fn delete_rows() -> Vec<Row> {
    vec![
        Row {
            name: "a",
            schema: json!({}),
        },
        Row {
            name: "logs-2026.01_v2",
            schema: json!({
                "mappings": {
                    "properties": {
                        "message": { "type": "text", "semantic_enrichment": { "enabled": true } }
                    }
                }
            }),
        },
        Row {
            name: "0metrics",
            schema: json!({
                "settings": { "index": { "number_of_shards": 2 } },
                "mappings": { "properties": { "title": { "type": "text" } } }
            }),
        },
        Row {
            name: ".hidden-index",
            schema: wide_schema(50),
        },
    ]
}

#[tokio::test]
async fn a_delete_removes_exactly_the_named_index_and_reports_deleted() {
    let emulator = Emulator::new();
    created_domain(&emulator, "delete-domain").await;
    created_domain(&emulator, "keeper-domain").await;

    let rows = delete_rows();
    // Both domains hold the whole table, so each delete has siblings beside it and a
    // same-named index on another domain to leave alone.
    for row in &rows {
        let case = format!("{:?}", row.name);
        created_index(&emulator, "delete-domain", row.name, &row.schema, &case).await;
        created_index(&emulator, "keeper-domain", row.name, &row.schema, &case).await;
    }

    let mut remaining: Vec<&str> = rows.iter().map(|row| row.name).collect();
    for row in &rows {
        let case = format!("{:?}", row.name);
        deleted_index(&emulator, "delete-domain", row.name, &case).await;
        remaining.retain(|name| *name != row.name);

        // The record is gone, so the modelled reader no longer finds it.
        let read = get(&emulator, "delete-domain", row.name).await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case GetIndex({case}, after the delete): expected {RECORDED_NOT_FOUND_STATUS} for a \
             deleted index, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case GetIndex({case}, after the delete): expected ResourceNotFoundException, got \
             {:?}",
            read.body
        );
        // The refusal's wording is fixed and its only variable part is the
        // `IndexName` the request carried.
        let message = string_member(&read.body, "message", &case);
        assert_eq!(
            message,
            format!("Resource index {} does not exist", row.name),
            "case GetIndex({case}, after the delete): unexpected not-found message, got \
             {message:?}"
        );

        // Only that one index went: the domain holds exactly the rows not yet deleted.
        let mut names = stored_names(&emulator, "delete-domain", &case);
        names.sort();
        let mut expected: Vec<String> = remaining.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        assert_eq!(
            names, expected,
            "case DeleteIndex({case}): expected the stored set {expected:?}, got {names:?}"
        );
    }

    // An index belongs to ONE domain: the other domain still holds every one of them,
    // with the schema its create carried.
    let mut kept = stored_names(&emulator, "keeper-domain", "keeper domain");
    kept.sort();
    let mut expected: Vec<String> = rows.iter().map(|row| row.name.to_string()).collect();
    expected.sort();
    assert_eq!(
        kept, expected,
        "case DeleteIndex(keeper domain): expected another domain's indexes untouched \
         {expected:?}, got {kept:?}"
    );
    for row in &rows {
        let case = format!("keeper-domain/{:?}", row.name);
        let read = get(&emulator, "keeper-domain", row.name).await;
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
}

#[tokio::test]
async fn deleting_in_any_order_leaves_exactly_the_indexes_not_deleted() {
    // Which index of a set a delete removes cannot depend on the order the deletes
    // arrive in, so every permutation of a three-index domain is driven.
    let trio = ["alpha", "beta.v1", "gamma-3"];
    let orders: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    for order in orders {
        let emulator = Emulator::new();
        created_domain(&emulator, "order-domain").await;
        for name in trio {
            created_index(
                &emulator,
                "order-domain",
                name,
                &json!({ "mappings": { "properties": { name: { "type": "keyword" } } } }),
                name,
            )
            .await;
        }

        let mut remaining: Vec<&str> = trio.to_vec();
        for step in order {
            let case = format!("order {order:?} removing {:?}", trio[step]);
            deleted_index(&emulator, "order-domain", trio[step], &case).await;
            remaining.retain(|name| *name != trio[step]);

            let mut names = stored_names(&emulator, "order-domain", &case);
            names.sort();
            let mut expected: Vec<String> =
                remaining.iter().map(|name| (*name).to_string()).collect();
            expected.sort();
            assert_eq!(
                names, expected,
                "case DeleteIndex({case}): expected the stored set {expected:?}, got {names:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_a_delete_freed_can_be_created_again() {
    let emulator = Emulator::new();
    created_domain(&emulator, "recreate-domain").await;

    // The name is the index's identity within its domain, so once the delete released
    // it a fresh create may take it — and what the second create persisted is what
    // reads back, not what the deleted index carried.
    for row in delete_rows() {
        let case = format!("{:?}", row.name);
        created_index(&emulator, "recreate-domain", row.name, &row.schema, &case).await;
        deleted_index(&emulator, "recreate-domain", row.name, &case).await;

        let replacement = json!({ "settings": { "index": { "knn": true } } });
        created_index(
            &emulator,
            "recreate-domain",
            row.name,
            &replacement,
            &format!("{case}, again"),
        )
        .await;

        let read = get(&emulator, "recreate-domain", row.name).await;
        assert_eq!(
            read.status, 200,
            "case GetIndex({case}, re-created): expected 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["IndexSchema"], replacement,
            "case GetIndex({case}, re-created): expected the schema the second create carried, \
             got {:?}",
            read.body["IndexSchema"]
        );

        // Leave the domain as it was found, so each row starts from the same store.
        deleted_index(&emulator, "recreate-domain", row.name, &format!("{case}, cleanup")).await;
    }
}

#[tokio::test]
async fn a_delete_is_refused_where_there_is_no_index_to_remove() {
    let emulator = Emulator::new();
    created_domain(&emulator, "refusal-domain").await;
    created_domain(&emulator, "released-domain").await;
    let released = delete_domain(&emulator, "released-domain").await;
    assert_eq!(
        released.status, 200,
        "case DeleteDomain(released-domain): expected 200, got {} with {:?}",
        released.status, released.body
    );
    created_index(
        &emulator,
        "refusal-domain",
        "present-index",
        &json!({ "mappings": {} }),
        "present",
    )
    .await;

    // A name no index could have is a malformed request, refused before the store is
    // consulted: one row per shape `IndexName`'s pattern excludes.
    for name in [
        ".", "..", "_leading", "-leading", "Upper", "has%20space", "has,comma", "has:colon",
    ] {
        let case = format!("DeleteIndex(refusal-domain/{name:?})");
        let answer = delete(&emulator, "refusal-domain", name).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400 outside the modelled constraint, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected ValidationException, got {:?}",
            answer.body
        );
    }

    // A domain name inside its own constraint that the store holds no domain for:
    // never created, and created then released.
    for domain in ["never-created", "released-domain"] {
        let case = format!("DeleteIndex({domain:?}/present-index)");
        let answer = delete(&emulator, domain, "present-index").await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected {RECORDED_NOT_FOUND_STATUS} for a domain the store does not \
             hold, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }

    // A domain the store DOES hold, and a name that names no index of it — including
    // one a character away from the index that IS there.
    for index in ["absent-index", "present-index.v2", "present"] {
        let case = format!("DeleteIndex(refusal-domain/{index:?})");
        let answer = delete(&emulator, "refusal-domain", index).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected {RECORDED_NOT_FOUND_STATUS} for a name that names no index, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        let message = string_member(&answer.body, "message", &case);
        assert_eq!(
            message,
            format!("Resource index {index} does not exist"),
            "case {case}: unexpected not-found message, got {message:?}"
        );
    }
    // Every refusal above removed nothing: the one index created is still there.
    let names = stored_names(&emulator, "refusal-domain", "after the refusals");
    assert_eq!(
        names,
        vec!["present-index".to_string()],
        "case DeleteIndex(after the refusals): a refused delete must remove nothing, got {names:?}"
    );

    // The second delete of one index is the same refusal: the first removed the record.
    deleted_index(&emulator, "refusal-domain", "present-index", "first delete").await;
    let again = delete(&emulator, "refusal-domain", "present-index").await;
    assert_eq!(
        again.status, RECORDED_NOT_FOUND_STATUS,
        "case DeleteIndex(present-index, again): expected {RECORDED_NOT_FOUND_STATUS} once the \
         record is gone, got {} with {:?}",
        again.status, again.body
    );
    assert_eq!(
        error_code(&again),
        "ResourceNotFoundException",
        "case DeleteIndex(present-index, again): expected ResourceNotFoundException, got {:?}",
        again.body
    );
}
