// SPDX-License-Identifier: CC0-1.0

//! `DeleteIndex`: what a delete removes from the collection's index store, end to end over
//! the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: a delete removes exactly the one resource its request addresses and
//! nothing else, so the following read reports that resource absent while every sibling and
//! every other collection still reports what its own create stored, and the freed name and
//! the freed room in the store are available again; a request addressing a resource the
//! store does not hold is the modelled not-found and removes nothing. Here the resource is
//! the index inside a collection, so the tables span the index-store shapes a delete runs
//! against — how many indexes the collection holds, which of them is addressed, the form of
//! the name and the shape of the schema it carried — rather than one request.

use serde_json::{json, Value};

use crate::collection_lifecycle::{call, emulator, text, CLIENT_ERROR};
use crate::encryption_indexes_and_tagging::active_collection;
use crate::index_not_found_wording::index_not_found_message;

/// The error code a collection or index the store does not hold carries
/// (`smithy_query(query="operation", operation="DeleteIndex")` -> errors).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code invalid input carries.
const VALIDATION: &str = "ValidationException";

/// How this service words a collection id that names no collection, on an index request.
fn collection_not_found_message(id: &str) -> String {
    format!("The Collection with Id '{id}' does not exist.")
}

/// `GetIndex` for one index: its HTTP status, error code and body.
async fn read_index(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
    index_name: &str,
) -> (u16, String, Value) {
    call(
        router,
        "GetIndex",
        json!({ "id": id, "indexName": index_name }),
    )
    .await
}

/// Create one index, asserting the create was accepted.
pub(crate) async fn create_index(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
    index_name: &str,
    schema: Option<&Value>,
    case: &str,
) {
    let mut request = json!({ "id": id, "indexName": index_name });
    if let Some(schema) = schema {
        request["indexSchema"] = schema.clone();
    }
    let (status, code, answer) = call(router, "CreateIndex", request).await;
    assert_eq!(
        status, 200,
        "case CreateIndex({case}, {index_name}): expected 200, got {status} {code} with {answer}"
    );
}

/// A schema naming `index_name`, so each index in a row carries one only it could have
/// stored.
fn schema_for(index_name: &str) -> Value {
    json!({ "mappings": { "properties": { index_name: { "type": "text" } } } })
}

/// (case, the names the collection's indexes are created under, the index the delete
/// addresses) — the rows span how many indexes the store holds when the delete runs, which
/// position in it is addressed (only, first, middle, last) and the forms an `IndexName`
/// takes under the rule its own member documents: lowercase letters, digits, an inner
/// hyphen or underscore, a dot, and a long name.
fn deletions() -> Vec<(&'static str, Vec<&'static str>, &'static str)> {
    vec![
        ("the only index", vec!["solo"], "solo"),
        (
            "the first of three",
            vec!["alpha", "beta", "gamma"],
            "alpha",
        ),
        (
            "the middle of three",
            vec!["alpha", "beta", "gamma"],
            "beta",
        ),
        ("the last of three", vec!["alpha", "beta", "gamma"], "gamma"),
        (
            "a name with an inner hyphen",
            vec!["orders-2026", "orders-2025"],
            "orders-2026",
        ),
        (
            "a name with an inner underscore",
            vec!["logs_v2", "logs_v1"],
            "logs_v2",
        ),
        (
            "a name with a dot",
            vec!["metrics.daily", "metrics.hourly"],
            "metrics.daily",
        ),
        ("a digits-only name", vec!["2026", "2025"], "2026"),
        (
            "a long name",
            vec![
                "a-very-long-index-name-that-is-still-perfectly-addressable",
                "short",
            ],
            "a-very-long-index-name-that-is-still-perfectly-addressable",
        ),
    ]
}

/// The delete removes the index it addresses and only that one: the read that follows
/// reports it absent, every sibling keeps the schema its create stored, the freed name can
/// be created again, and a second delete of the same name is the modelled not-found.
#[tokio::test]
async fn a_delete_removes_only_the_index_it_addresses() {
    let router = emulator();

    for (position, (case, names, target)) in deletions().into_iter().enumerate() {
        // One collection per row, so a row's index store is exactly the names it names.
        let collection = active_collection(&router, &format!("deleted-indexes-{position}")).await;
        let id = text(&collection, "id");

        for name in &names {
            create_index(&router, &id, name, Some(&schema_for(name)), case).await;
        }

        let (status, code, answer) = call(
            &router,
            "DeleteIndex",
            json!({ "id": &id, "indexName": target }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case DeleteIndex({case}): expected 200, got {status} {code} with {answer}"
        );

        // The addressed index is gone, and the refusal names the index asked for.
        let (status, code, answer) = read_index(&router, &id, target).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, NOT_FOUND),
            "case GetIndex({case}, after the delete): expected {CLIENT_ERROR} {NOT_FOUND}, got \
             {status} {code} with {answer}"
        );
        let got_message = text(&answer, "message");
        let expected_message = index_not_found_message(target);
        assert_eq!(
            got_message, expected_message,
            "case GetIndex({case}, after the delete): expected {expected_message:?}, got \
             {got_message:?}"
        );

        // Every sibling still reports the schema its own create stored.
        for sibling in names.iter().filter(|name| **name != target) {
            let (status, code, answer) = read_index(&router, &id, sibling).await;
            assert_eq!(
                status, 200,
                "case GetIndex({case}, sibling {sibling}): expected 200, got {status} {code} \
                 with {answer}"
            );
            let got = answer.get("indexSchema").cloned();
            assert_eq!(
                got.as_ref(),
                Some(&schema_for(sibling)),
                "case GetIndex({case}, sibling {sibling}): expected the stored schema {:?}, got \
                 {got:?}",
                schema_for(sibling)
            );
        }

        // A second delete of the same name addresses an index the store no longer holds:
        // `@idempotent` makes the retry safe to send, not successful.
        let (status, code, answer) = call(
            &router,
            "DeleteIndex",
            json!({ "id": &id, "indexName": target }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, NOT_FOUND),
            "case DeleteIndex({case}, replayed): expected {CLIENT_ERROR} {NOT_FOUND}, got \
             {status} {code} with {answer}"
        );
        let got_message = text(&answer, "message");
        assert_eq!(
            got_message, expected_message,
            "case DeleteIndex({case}, replayed): expected {expected_message:?}, got \
             {got_message:?}"
        );

        // The freed name is available again, and what the new create stores is what the
        // read reports — so the delete removed the record rather than hiding it.
        let recreated = json!({ "mappings": { "properties": { "recreated": { "type": "keyword" } } } });
        create_index(&router, &id, target, Some(&recreated), case).await;
        let (status, code, answer) = read_index(&router, &id, target).await;
        assert_eq!(
            status, 200,
            "case GetIndex({case}, recreated): expected 200, got {status} {code} with {answer}"
        );
        let got = answer.get("indexSchema").cloned();
        assert_eq!(
            got.as_ref(),
            Some(&recreated),
            "case GetIndex({case}, recreated): expected {recreated}, got {got:?}"
        );
    }
}

/// The delete reaches only the collection its `id` names: an index of the same name in
/// another collection is untouched.
#[tokio::test]
async fn a_delete_reaches_only_the_collection_its_id_names() {
    let router = emulator();
    let first = active_collection(&router, "index-delete-first").await;
    let second = active_collection(&router, "index-delete-second").await;
    let first_id = text(&first, "id");
    let second_id = text(&second, "id");

    // The SAME index name in two collections, each carrying a schema naming its collection.
    let shared_name = "shared";
    let first_schema = json!({ "mappings": { "properties": { "first": { "type": "text" } } } });
    let second_schema = json!({ "mappings": { "properties": { "second": { "type": "text" } } } });
    create_index(&router, &first_id, shared_name, Some(&first_schema), "two collections").await;
    create_index(
        &router,
        &second_id,
        shared_name,
        Some(&second_schema),
        "two collections",
    )
    .await;

    let (status, code, answer) = call(
        &router,
        "DeleteIndex",
        json!({ "id": &first_id, "indexName": shared_name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteIndex(two collections): expected 200, got {status} {code} with {answer}"
    );

    let (status, code, answer) = read_index(&router, &first_id, shared_name).await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, NOT_FOUND),
        "case GetIndex(the addressed collection): expected {CLIENT_ERROR} {NOT_FOUND}, got \
         {status} {code} with {answer}"
    );

    let (status, code, answer) = read_index(&router, &second_id, shared_name).await;
    assert_eq!(
        status, 200,
        "case GetIndex(the other collection): expected 200, got {status} {code} with {answer}"
    );
    let got = answer.get("indexSchema").cloned();
    assert_eq!(
        got.as_ref(),
        Some(&second_schema),
        "case GetIndex(the other collection): expected {second_schema}, got {got:?}"
    );
}

/// What the delete refuses, and that a refused delete removes nothing.
#[tokio::test]
async fn a_delete_that_addresses_nothing_is_refused_and_removes_nothing() {
    let router = emulator();
    let collection = active_collection(&router, "refused-index-deletes").await;
    let id = text(&collection, "id");
    // Matches `CollectionId`'s `@pattern`, so it reaches the handler, and names no
    // collection in the store.
    let absent_id = "0".repeat(20);

    let kept = schema_for("present");
    create_index(&router, &id, "present", Some(&kept), "refusals").await;

    // (case, request, expected code, expected message) — the rows span the classes of
    // refusal: the collection is absent, the index is absent, and the name breaks the rule
    // its own member documents ("Index names must be lowercase and can't begin with
    // underscores (_) or hyphens (-)", `smithy_query(query="doc",
    // shape="CreateIndexRequest", member="indexName")`), on each way it can break it.
    let refusals: Vec<(&str, Value, &str, Option<String>)> = vec![
        (
            "a collection id that names no collection",
            json!({ "id": &absent_id, "indexName": "present" }),
            NOT_FOUND,
            Some(collection_not_found_message(&absent_id)),
        ),
        (
            "an index name that names no index",
            json!({ "id": &id, "indexName": "never-created" }),
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
            json!({ "id": &id, "indexName": "Present" }),
            VALIDATION,
            None,
        ),
        (
            "an empty index name",
            json!({ "id": &id, "indexName": "" }),
            VALIDATION,
            None,
        ),
    ];

    for (case, request, expected_code, expected_message) in refusals {
        let (status, code, answer) = call(&router, "DeleteIndex", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, expected_code),
            "case DeleteIndex({case}): expected {CLIENT_ERROR} {expected_code}, got {status} \
             {code} with {answer}"
        );
        if let Some(expected_message) = expected_message {
            let got_message = text(&answer, "message");
            assert_eq!(
                got_message, expected_message,
                "case DeleteIndex({case}): expected {expected_message:?}, got {got_message:?}"
            );
        }

        // The index the store does hold is still there with the schema its create stored,
        // so no refused delete removed anything.
        let (status, code, answer) = read_index(&router, &id, "present").await;
        assert_eq!(
            status, 200,
            "case GetIndex({case}, the present index): expected 200, got {status} {code} with \
             {answer}"
        );
        let got = answer.get("indexSchema").cloned();
        assert_eq!(
            got.as_ref(),
            Some(&kept),
            "case GetIndex({case}, the present index): expected the stored schema {kept}, got \
             {got:?}"
        );
    }
}

/// The delete frees room in the per-collection index store: at the emulator's own bound a
/// create is refused, and after one delete the next create is accepted again — so the
/// delete removes the entry from the store the bound counts, not just from the read path.
#[tokio::test]
async fn a_delete_frees_room_in_the_bounded_index_store() {
    use service_com_amazonaws_opensearchserverless_opensearchserverless::state::MAX_INDEXES_PER_COLLECTION;

    let router = emulator();
    let collection = active_collection(&router, "index-delete-capacity").await;
    let id = text(&collection, "id");

    for index in 0..MAX_INDEXES_PER_COLLECTION {
        create_index(&router, &id, &format!("index-{index}"), None, "at the cap").await;
    }

    let (status, code, answer) = call(
        &router,
        "CreateIndex",
        json!({ "id": &id, "indexName": "one-index-too-many" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (CLIENT_ERROR, VALIDATION),
        "case CreateIndex(at the cap): expected {CLIENT_ERROR} {VALIDATION}, got {status} {code} \
         with {answer}"
    );

    let (status, code, answer) = call(
        &router,
        "DeleteIndex",
        json!({ "id": &id, "indexName": "index-0" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteIndex(at the cap): expected 200, got {status} {code} with {answer}"
    );

    let (status, code, answer) = call(
        &router,
        "CreateIndex",
        json!({ "id": &id, "indexName": "one-index-too-many" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateIndex(after the delete freed room): expected 200, got {status} {code} with \
         {answer}"
    );
}
