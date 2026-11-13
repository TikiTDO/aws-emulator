// SPDX-License-Identifier: CC0-1.0

//! `UpdateIndex`: the stored schema an update leaves, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: an update of an existing resource applies the members its request
//! NAMES to the stored record and leaves every member it omits as it was, so the following
//! read reports what the update wrote; a request addressing a resource the store does not
//! hold is the modelled not-found and writes nothing. Here the resource is the index inside
//! a collection and the member is `indexSchema`, "the updated JSON schema definition for
//! the index" (`smithy_query(query="doc", shape="UpdateIndexRequest",
//! member="indexSchema")`), which is a document — so the tables span the SHAPES a document
//! takes and the directions one shape can be replaced by another, rather than the one
//! request the issue quoted.

use serde_json::{json, Value};

use crate::collection_lifecycle::{call, emulator, text, CLIENT_ERROR};
use crate::encryption_indexes_and_tagging::active_collection;
use crate::index_not_found_wording::index_not_found_message;

/// The error code an index or collection the store does not hold carries
/// (`smithy_query(query="operation", operation="UpdateIndex")` -> errors).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code invalid input carries.
const VALIDATION: &str = "ValidationException";

/// How this service words a collection id that names no collection, on an index request.
fn collection_not_found_message(id: &str) -> String {
    format!("The Collection with Id '{id}' does not exist.")
}

/// The `indexSchema` `GetIndex` reports for `index_name`, asserting the read succeeded.
pub(crate) async fn stored_schema(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
    index_name: &str,
    case: &str,
) -> Option<Value> {
    let (status, code, read) = call(
        router,
        "GetIndex",
        json!({ "id": id, "indexName": index_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case GetIndex({case}): expected 200, got {status} {code} with {read}"
    );
    read.get("indexSchema").cloned()
}

/// A schema nesting `levels` deep under one member per level, so a row can sit at a chosen
/// depth.
fn nested(levels: usize) -> Value {
    let mut document = json!("leaf");
    for _ in 0..levels {
        document = json!({ "properties": document });
    }
    document
}

/// A schema of `values` sibling members, which nests only one level.
fn wide(values: usize) -> Value {
    let mut members = serde_json::Map::new();
    for value in 0..values {
        members.insert(format!("field-{value}"), json!("keyword"));
    }
    Value::Object(members)
}

/// (case, the schema the create stored, the schema the update names) — the rows span the
/// document shapes an `indexSchema` takes and the directions one is replaced by another:
/// a field added, a mapping changed, a container swapped for a scalar and back, a list
/// reordered, a schema emptied, and an index that had no schema at all gaining one.
fn replacements() -> Vec<(&'static str, Option<Value>, Value)> {
    vec![
        (
            "a field added to a mappings object",
            Some(json!({ "mappings": { "properties": { "title": { "type": "text" } } } })),
            json!({
                "mappings": {
                    "properties": {
                        "title": { "type": "text" },
                        "published": { "type": "date" },
                    },
                },
            }),
        ),
        (
            "a field mapping changed",
            Some(json!({ "mappings": { "properties": { "title": { "type": "text" } } } })),
            json!({ "mappings": { "properties": { "title": { "type": "keyword" } } } }),
        ),
        (
            "settings and a vector field added beside the mappings",
            Some(json!({ "mappings": { "properties": { "tags": { "type": "keyword" } } } })),
            json!({
                "settings": { "index": { "number_of_shards": 2 } },
                "mappings": {
                    "properties": {
                        "tags": { "type": "keyword" },
                        "embedding": { "type": "knn_vector", "dimension": 1536 },
                    },
                },
            }),
        ),
        (
            "an object replaced by a scalar document",
            Some(json!({ "mappings": { "properties": {} } })),
            json!("text"),
        ),
        (
            "a scalar document replaced by an object",
            Some(json!("text")),
            json!({ "mappings": { "properties": { "body": { "type": "text" } } } }),
        ),
        (
            "a list reordered",
            Some(json!({ "aliases": ["read", "write"] })),
            json!({ "aliases": ["write", "read"] }),
        ),
        (
            "a schema emptied",
            Some(json!({ "mappings": { "properties": { "title": { "type": "text" } } } })),
            json!({}),
        ),
        (
            "an index created without a schema gaining one",
            None,
            json!({ "mappings": { "properties": { "body": { "type": "text" } } } }),
        ),
        (
            "a schema at the emulator's depth bound",
            Some(json!({ "mappings": {} })),
            nested(8),
        ),
    ]
}

#[tokio::test]
async fn an_update_replaces_the_stored_schema_and_the_read_reports_it() {
    let router = emulator();
    let collection = active_collection(&router, "updated-indexes").await;
    let id = text(&collection, "id");

    for (position, (case, created, updated)) in replacements().into_iter().enumerate() {
        let index_name = format!("updated-index-{position}");

        let mut create = json!({ "id": &id, "indexName": &index_name });
        if let Some(schema) = &created {
            create["indexSchema"] = schema.clone();
        }
        let (status, code, answer) = call(&router, "CreateIndex", create).await;
        assert_eq!(
            status, 200,
            "case CreateIndex({case}): expected 200, got {status} {code} with {answer}"
        );

        let (status, code, answer) = call(
            &router,
            "UpdateIndex",
            json!({ "id": &id, "indexName": &index_name, "indexSchema": &updated }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case UpdateIndex({case}): expected 200, got {status} {code} with {answer}"
        );

        let got = stored_schema(&router, &id, &index_name, case).await;
        assert_eq!(
            got.as_ref(),
            Some(&updated),
            "case GetIndex({case}, after the update): expected the schema the update named, \
             {updated}, got {got:?}"
        );

        // `UpdateIndex` is `@idempotent`: the same update again lands the same state.
        let (status, code, answer) = call(
            &router,
            "UpdateIndex",
            json!({ "id": &id, "indexName": &index_name, "indexSchema": &updated }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case UpdateIndex({case}, replayed): expected 200, got {status} {code} with {answer}"
        );
        let got = stored_schema(&router, &id, &index_name, case).await;
        assert_eq!(
            got.as_ref(),
            Some(&updated),
            "case GetIndex({case}, after the replay): expected {updated}, got {got:?}"
        );

        // A request naming NO `indexSchema` names no schema change, so it leaves the one
        // the update wrote exactly as it was.
        let (status, code, answer) = call(
            &router,
            "UpdateIndex",
            json!({ "id": &id, "indexName": &index_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case UpdateIndex({case}, no indexSchema): expected 200, got {status} {code} with {answer}"
        );
        let got = stored_schema(&router, &id, &index_name, case).await;
        assert_eq!(
            got.as_ref(),
            Some(&updated),
            "case GetIndex({case}, after an update naming no schema): expected the schema left \
             untouched, {updated}, got {got:?}"
        );

        // The update writes only the index it addresses: a SIBLING index in the same
        // collection keeps its own schema.
        let sibling = format!("sibling-index-{position}");
        let sibling_schema = json!({ "mappings": { "properties": { &sibling: { "type": "keyword" } } } });
        let (status, code, answer) = call(
            &router,
            "CreateIndex",
            json!({ "id": &id, "indexName": &sibling, "indexSchema": &sibling_schema }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateIndex({case}, sibling): expected 200, got {status} {code} with {answer}"
        );
        let (status, code, answer) = call(
            &router,
            "UpdateIndex",
            json!({ "id": &id, "indexName": &index_name, "indexSchema": &updated }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case UpdateIndex({case}, sibling present): expected 200, got {status} {code} with {answer}"
        );
        let got = stored_schema(&router, &id, &sibling, case).await;
        assert_eq!(
            got.as_ref(),
            Some(&sibling_schema),
            "case GetIndex({case}, sibling after the update): expected {sibling_schema}, got {got:?}"
        );
    }
}

/// Successive updates of ONE index: each read reports the schema the LAST update wrote, so
/// the stored schema is replaced rather than accumulated.
#[tokio::test]
async fn successive_updates_each_replace_what_the_previous_one_wrote() {
    let router = emulator();
    let collection = active_collection(&router, "resequenced-index").await;
    let id = text(&collection, "id");
    let index_name = "resequenced";

    let (status, code, answer) = call(
        &router,
        "CreateIndex",
        json!({
            "id": &id,
            "indexName": index_name,
            "indexSchema": { "mappings": { "properties": { "v": { "type": "integer" } } } },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateIndex(resequenced): expected 200, got {status} {code} with {answer}"
    );

    let sequence = [
        json!({ "mappings": { "properties": { "v": { "type": "long" } } } }),
        json!({ "mappings": { "properties": { "v": { "type": "long" }, "w": { "type": "text" } } } }),
        json!({ "mappings": { "properties": { "w": { "type": "text" } } } }),
        json!({}),
        json!(42),
    ];
    for (step, schema) in sequence.iter().enumerate() {
        let case = format!("step {step}");
        let (status, code, answer) = call(
            &router,
            "UpdateIndex",
            json!({ "id": &id, "indexName": index_name, "indexSchema": schema }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case UpdateIndex({case}): expected 200, got {status} {code} with {answer}"
        );
        let got = stored_schema(&router, &id, index_name, &case).await;
        assert_eq!(
            got.as_ref(),
            Some(schema),
            "case GetIndex({case}): expected only the last schema written, {schema}, got {got:?}"
        );
    }
}

/// What the update refuses, and that a refused update leaves the store exactly as it was —
/// including never creating the index a request addresses but the store does not hold.
#[tokio::test]
async fn an_update_that_addresses_no_index_is_refused_and_writes_nothing() {
    use service_com_amazonaws_opensearchserverless_opensearchserverless::state::{
        MAX_DOCUMENT_DEPTH, MAX_INDEX_SCHEMA_NODES,
    };

    let router = emulator();
    let collection = active_collection(&router, "refused-updates").await;
    let id = text(&collection, "id");
    // Matches `CollectionId`'s `@pattern`, so it reaches the handler, and names no
    // collection in the store.
    let absent_id = "0".repeat(20);

    let kept = json!({ "mappings": { "properties": { "title": { "type": "text" } } } });
    let (status, code, answer) = call(
        &router,
        "CreateIndex",
        json!({ "id": &id, "indexName": "present", "indexSchema": &kept }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateIndex(present): expected 200, got {status} {code} with {answer}"
    );

    // (case, request, expected code, expected message) — the rows span the classes of
    // refusal: the collection is absent, the index is absent, the name breaks the rule its
    // own member documents, and the schema is past what this emulator stores.
    let refusals: Vec<(&str, Value, &str, Option<String>)> = vec![
        (
            "a collection id that names no collection",
            json!({ "id": &absent_id, "indexName": "present", "indexSchema": { "a": 1 } }),
            NOT_FOUND,
            Some(collection_not_found_message(&absent_id)),
        ),
        (
            "an index name that names no index",
            json!({ "id": &id, "indexName": "never-created", "indexSchema": { "a": 1 } }),
            NOT_FOUND,
            Some(index_not_found_message("never-created")),
        ),
        (
            "an index name beginning with an underscore",
            json!({ "id": &id, "indexName": "_system-index" }),
            VALIDATION,
            None,
        ),
        (
            "an index name beginning with a hyphen",
            json!({ "id": &id, "indexName": "-leading-hyphen" }),
            VALIDATION,
            None,
        ),
        (
            "an index name with an uppercase letter",
            json!({ "id": &id, "indexName": "MixedCase" }),
            VALIDATION,
            None,
        ),
        (
            "an empty index name",
            json!({ "id": &id, "indexName": "" }),
            VALIDATION,
            None,
        ),
        (
            "a schema one level past the depth bound",
            json!({
                "id": &id,
                "indexName": "present",
                "indexSchema": nested(MAX_DOCUMENT_DEPTH + 1),
            }),
            VALIDATION,
            None,
        ),
        (
            "a schema one value past the value bound",
            json!({
                "id": &id,
                "indexName": "present",
                "indexSchema": wide(MAX_INDEX_SCHEMA_NODES + 1),
            }),
            VALIDATION,
            None,
        ),
    ];

    for (case, request, expected_code, expected_message) in refusals {
        let (status, code, answer) = call(&router, "UpdateIndex", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, expected_code),
            "case UpdateIndex({case}): expected {CLIENT_ERROR} {expected_code}, got {status} \
             {code} with {answer}"
        );
        if let Some(expected_message) = expected_message {
            let got_message = text(&answer, "message");
            assert_eq!(
                got_message, expected_message,
                "case UpdateIndex({case}): expected {expected_message:?}, got {got_message:?}"
            );
        }

        // Nothing the refused request named reached the store: the index it addressed by a
        // name that was never created still does not exist...
        let (status, code, read) = call(
            &router,
            "GetIndex",
            json!({ "id": &id, "indexName": "never-created" }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, NOT_FOUND),
            "case GetIndex({case}, the absent index): a refused update creates nothing, got \
             {status} {code} with {read}"
        );
        // ...and the index that does keeps the schema the create stored.
        let got = stored_schema(&router, &id, "present", case).await;
        assert_eq!(
            got.as_ref(),
            Some(&kept),
            "case GetIndex({case}, the present index): expected the stored schema {kept}, got \
             {got:?}"
        );
    }

    // A schema AT each bound is admitted, so the refusals above narrow nothing the update
    // must accept.
    for (case, schema) in [
        ("at the depth bound", nested(MAX_DOCUMENT_DEPTH - 2)),
        ("at the value bound", wide(MAX_INDEX_SCHEMA_NODES - 1)),
    ] {
        let (status, code, answer) = call(
            &router,
            "UpdateIndex",
            json!({ "id": &id, "indexName": "present", "indexSchema": &schema }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case UpdateIndex(a schema {case}): expected 200, got {status} {code} with {answer}"
        );
        let got = stored_schema(&router, &id, "present", case).await;
        assert_eq!(
            got.as_ref(),
            Some(&schema),
            "case GetIndex(a schema {case}): expected the schema the update named, got {got:?}"
        );
    }
}
