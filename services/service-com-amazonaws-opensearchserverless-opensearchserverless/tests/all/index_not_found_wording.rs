// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS an index request that addresses no index, end to end over the
//! wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: an index request whose `indexName` addresses no index in a
//! collection the request's `id` DOES name is the modelled `ResourceNotFoundException`
//! (`smithy_query(query="operation", operation="GetIndex,UpdateIndex,DeleteIndex")` ->
//! errors), and its message is the fixed text `Index not found: ` followed by exactly one
//! variable part, the request's `indexName` — nothing else, and in particular nothing read
//! out of state, so the message a client matches on depends only on what it asked for. An
//! `id` that names no collection is a DIFFERENT refusal and keeps wording itself, naming
//! the collection.
//!
//! The tables span what the rule generalises over: every operation that addresses an index
//! by name, how the index came to be absent (never created, or created and then deleted),
//! the forms an `IndexName` takes under the rule its own member documents, and the
//! collection the request addresses — the same absent name in two collections with
//! different ids must be worded identically.

use serde_json::{json, Value};

use crate::collection_lifecycle::{call, emulator, text, CLIENT_ERROR};
use crate::encryption_indexes_and_tagging::active_collection;
use crate::index_delete::create_index;

/// The error code an index the store does not hold carries
/// (`smithy_query(query="operation", operation="GetIndex")` -> errors).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// How this service words an index that is not there: the fixed text plus the `indexName`
/// the request carried. The one expected wording the suites in this crate compare against.
pub(crate) fn index_not_found_message(index_name: &str) -> String {
    format!("Index not found: {index_name}")
}

/// How this service words a collection id that names no collection, on an index request —
/// the neighbouring refusal, which keeps naming the collection.
fn collection_not_found_message(id: &str) -> String {
    format!("The Collection with Id '{id}' does not exist.")
}

/// A schema an update may carry, so `UpdateIndex` is refused for the index it addresses and
/// not for its payload.
fn schema() -> Value {
    json!({ "mappings": { "properties": { "title": { "type": "text" } } } })
}

/// Every operation that addresses an index by `(id, indexName)`, as the request each sends.
fn index_requests(id: &str, index_name: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("GetIndex", json!({ "id": id, "indexName": index_name })),
        (
            "UpdateIndex",
            json!({ "id": id, "indexName": index_name, "indexSchema": schema() }),
        ),
        ("DeleteIndex", json!({ "id": id, "indexName": index_name })),
    ]
}

/// (case, the index name the request carries, whether the index is created and then deleted
/// before the probes) — the rows differ in the form of the name (length, inner punctuation,
/// digits, script) under the rule `CreateIndexRequest$indexName` documents ("Index names
/// must be lowercase and can't begin with underscores (_) or hyphens (-)",
/// `smithy_query(query="doc", shape="CreateIndexRequest", member="indexName")`), and in how
/// the name came to address nothing.
fn absent_names() -> Vec<(&'static str, String, bool)> {
    vec![
        ("a plain name never created", "logs".to_string(), false),
        ("a plain name once created", "orders".to_string(), true),
        (
            "a name with an inner hyphen, once created",
            "app-logs-2026".to_string(),
            true,
        ),
        (
            "a name with an inner underscore, never created",
            "app_logs".to_string(),
            false,
        ),
        (
            "a name with a dot, once created",
            "metrics.daily".to_string(),
            true,
        ),
        ("a digits-only name", "2026".to_string(), true),
        ("a single-letter name", "a".to_string(), false),
        ("a lowercase non-ASCII name", "индекс".to_string(), true),
        ("a long name", "l".repeat(255), false),
    ]
}

/// Whatever the operation, the form of the name and the collection addressed, an absent
/// index is refused with the one wording, and that wording carries the requested name and
/// nothing else.
#[tokio::test]
async fn an_absent_index_is_worded_from_the_requested_name_alone() {
    let router = emulator();
    // Two collections, so the same absent name is probed under two different ids: a message
    // that leaked the id would differ between them.
    let first = active_collection(&router, "index-not-found-first").await;
    let second = active_collection(&router, "index-not-found-second").await;
    let first_id = text(&first, "id");
    let second_id = text(&second, "id");

    for (case, name, once_created) in absent_names() {
        if once_created {
            create_index(&router, &first_id, &name, Some(&schema()), case).await;
            let (status, code, answer) = call(
                &router,
                "DeleteIndex",
                json!({ "id": &first_id, "indexName": &name }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case DeleteIndex({case}, the setup delete): expected 200, got {status} {code} \
                 with {answer}"
            );
        }

        let expected_message = index_not_found_message(&name);
        for id in [&first_id, &second_id] {
            for (operation, request) in index_requests(id, &name) {
                let (status, code, answer) = call(&router, operation, request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (CLIENT_ERROR, NOT_FOUND),
                    "case {operation}({case}, collection {id}): expected {CLIENT_ERROR} \
                     {NOT_FOUND}, got {status} {code} with {answer}"
                );
                let got_message = text(&answer, "message");
                assert_eq!(
                    got_message, expected_message,
                    "case {operation}({case}, collection {id}): expected {expected_message:?}, \
                     got {got_message:?}"
                );
                assert!(
                    !got_message.contains(id.as_str()),
                    "case {operation}({case}, collection {id}): the wording must carry the \
                     requested name alone, got {got_message:?}"
                );
            }
        }

        // The refusal is about the index being absent, not a blanket answer for the name:
        // once created, the same request reports the stored schema.
        create_index(&router, &first_id, &name, Some(&schema()), case).await;
        let (status, code, answer) = call(
            &router,
            "GetIndex",
            json!({ "id": &first_id, "indexName": &name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case GetIndex({case}, created again): expected 200, got {status} {code} with {answer}"
        );
        let got = answer.get("indexSchema").cloned();
        assert_eq!(
            got.as_ref(),
            Some(&schema()),
            "case GetIndex({case}, created again): expected the stored schema {}, got {got:?}",
            schema()
        );
    }
}

/// An `id` that names no collection is the neighbouring refusal: it keeps naming the
/// collection and is NOT the index wording, so a client can still tell the two apart.
#[tokio::test]
async fn an_absent_collection_keeps_wording_its_own_refusal() {
    let router = emulator();
    let collection = active_collection(&router, "index-not-found-absent-collection").await;
    let id = text(&collection, "id");
    create_index(&router, &id, "present", Some(&schema()), "absent collection").await;

    // Ids matching `CollectionId`'s modelled `^[a-z0-9]{3,40}$`, at its length bounds and
    // between them, so each reaches the handler and names no collection in the store.
    let absent_ids = ["0".repeat(3), "0".repeat(20), "0".repeat(40)];
    for absent_id in absent_ids {
        let expected_message = collection_not_found_message(&absent_id);
        for (operation, request) in index_requests(&absent_id, "present") {
            let (status, code, answer) = call(&router, operation, request).await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, NOT_FOUND),
                "case {operation}(absent collection {absent_id}): expected {CLIENT_ERROR} \
                 {NOT_FOUND}, got {status} {code} with {answer}"
            );
            let got_message = text(&answer, "message");
            assert_eq!(
                got_message, expected_message,
                "case {operation}(absent collection {absent_id}): expected \
                 {expected_message:?}, got {got_message:?}"
            );
        }
    }

    // The index that IS there under the id that IS a collection is untouched by all of it.
    let (status, code, answer) = call(
        &router,
        "GetIndex",
        json!({ "id": &id, "indexName": "present" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case GetIndex(the present index): expected 200, got {status} {code} with {answer}"
    );
}
