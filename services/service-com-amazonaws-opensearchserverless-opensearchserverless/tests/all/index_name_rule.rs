// SPDX-License-Identifier: CC0-1.0

//! The index-name rule, end to end over the wire, on the READ as well as on the write.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: a documented constraint on a modelled input member belongs to the
//! MEMBER, so every operation taking that member refuses a value violating it as the
//! invalid input `ValidationException` describes, with one wording, before resolving the
//! resource the request addresses — a read therefore never reports an illegal value as
//! merely absent. Here the member is `indexName` and the constraint is the one its own
//! documentation states: "Index names must be lowercase and can't begin with underscores
//! (_) or hyphens (-)" (`smithy_query(query="doc", shape="CreateIndexRequest",
//! member="indexName")`), with the empty name refused too since an index has to be named
//! to be addressable at all.
//!
//! The tables span what the rule generalises over — which way a name violates it and where
//! in the name the violation sits, and, on the admitting side, the length, script and inner
//! punctuation of a legal name — rather than the one request the issue quoted. The refusal
//! wording is compared against the SIBLING operation's rather than respelled here, since
//! "one wording" is the part of the rule that was broken.

use serde_json::{json, Value};

use crate::collection_lifecycle::{call, emulator, text, CLIENT_ERROR};
use crate::encryption_indexes_and_tagging::active_collection;
use crate::index_not_found_wording::index_not_found_message;

/// The error code a value the rule refuses carries
/// (`smithy_query(query="shape", shape="ValidationException")`).
const VALIDATION: &str = "ValidationException";

/// The error code a legal name that names no index carries.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// An index schema naming one field after the index it belongs to, so each row round-trips
/// a schema of its own rather than one shared document.
fn schema_for(index_name: &str) -> Value {
    let mut schema = json!({ "mappings": { "properties": {} } });
    schema["mappings"]["properties"][index_name] = json!({ "type": "keyword" });
    schema
}

/// Every way an `indexName` can fail the documented rule: not lowercase, or beginning with
/// an underscore or a hyphen, plus the empty name. The rows differ in WHICH clause they
/// fail and WHERE in the name they fail it.
fn refused_names() -> Vec<(&'static str, String)> {
    vec![
        ("a leading underscore", "_system-index".to_string()),
        ("a leading hyphen", "-leading-hyphen".to_string()),
        ("nothing but an underscore", "_".to_string()),
        ("nothing but a hyphen", "-".to_string()),
        ("an uppercase first letter", "Mixedcase".to_string()),
        ("an uppercase letter in the middle", "mixedCase".to_string()),
        ("an uppercase final letter", "mixedcasE".to_string()),
        ("an all-uppercase name", "LOGS".to_string()),
        // Case is a property of the character, not of ASCII: a non-ASCII capital fails the
        // same clause.
        ("an uppercase non-ASCII letter", "logs-Ünd".to_string()),
        ("the empty name", String::new()),
    ]
}

/// Names the rule ADMITS: non-empty, lowercase, beginning with neither `_` nor `-`. The
/// rows differ in length, script and inner punctuation. `IndexName` carries no `@length`
/// and no `@pattern` (`smithy_query(query="operation", operation="CreateIndex")`), so
/// nothing narrows them further.
fn admitted_names() -> Vec<(&'static str, String)> {
    vec![
        ("a plain lowercase name", "logs".to_string()),
        ("a single lowercase letter", "a".to_string()),
        ("an inner underscore", "app_logs".to_string()),
        ("an inner hyphen", "app-logs-2026".to_string()),
        ("a trailing underscore", "logs_".to_string()),
        ("digits only", "2026".to_string()),
        ("a lowercase non-ASCII name", "индекс".to_string()),
        ("a long name", "l".repeat(255)),
    ]
}

#[tokio::test]
async fn an_index_name_the_rule_refuses_is_invalid_input_on_the_read_too() {
    let router = emulator();
    let collection = active_collection(&router, "index-name-rule").await;
    let id = text(&collection, "id");
    // Matches `CollectionId`'s `@pattern`, so it reaches the handler, and names no
    // collection in the store.
    let absent_id = "0".repeat(20);

    for (case, name) in refused_names() {
        // The write refuses it...
        let (status, code, written) = call(
            &router,
            "CreateIndex",
            json!({ "id": &id, "indexName": &name }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, VALIDATION),
            "case CreateIndex({case}): expected {CLIENT_ERROR} {VALIDATION}, got {status} {code} with {written}"
        );

        // ...and so does the read, with the SAME wording: one member, one rule, one
        // refusal, so a client cannot be told the name is illegal by one operation and
        // merely unused by the other.
        let (status, code, read) =
            call(&router, "GetIndex", json!({ "id": &id, "indexName": &name })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, VALIDATION),
            "case GetIndex({case}): expected {CLIENT_ERROR} {VALIDATION}, got {status} {code} with {read}"
        );
        let expected_message = text(&written, "message");
        let got_message = text(&read, "message");
        assert_eq!(
            got_message, expected_message,
            "case GetIndex({case}): expected the refusal the write words, {expected_message:?}, got {got_message:?}"
        );
        assert!(
            !got_message.is_empty(),
            "case GetIndex({case}): expected the refusal to carry a message, got {read}"
        );

        // The value is invalid input, so it is refused BEFORE the collection it addresses
        // is resolved: an id naming no collection answers the same refusal, not a
        // not-found about the collection.
        let (status, code, read) = call(
            &router,
            "GetIndex",
            json!({ "id": &absent_id, "indexName": &name }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, VALIDATION),
            "case GetIndex({case}, collection absent too): expected {CLIENT_ERROR} {VALIDATION}, got {status} {code} with {read}"
        );
    }
}

#[tokio::test]
async fn a_name_the_rule_admits_is_absent_until_created_and_then_round_trips() {
    let router = emulator();
    let collection = active_collection(&router, "legal-index-names").await;
    let id = text(&collection, "id");

    for (case, name) in admitted_names() {
        // A legal name that names no index is the modelled not-found, naming the index the
        // request asked for.
        let (status, code, read) =
            call(&router, "GetIndex", json!({ "id": &id, "indexName": &name })).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, NOT_FOUND),
            "case GetIndex({case}, before the create): expected {CLIENT_ERROR} {NOT_FOUND}, got {status} {code} with {read}"
        );
        let expected_message = index_not_found_message(&name);
        let got_message = text(&read, "message");
        assert_eq!(
            got_message, expected_message,
            "case GetIndex({case}, before the create): expected {expected_message:?}, got {got_message:?}"
        );

        // The same name is accepted by the write, and the read then reports the stored
        // schema: the validation added to the read narrows nothing it must admit.
        let schema = schema_for(&name);
        let (status, code, written) = call(
            &router,
            "CreateIndex",
            json!({ "id": &id, "indexName": &name, "indexSchema": &schema }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateIndex({case}): expected 200, got {status} {code} with {written}"
        );
        let (status, code, read) =
            call(&router, "GetIndex", json!({ "id": &id, "indexName": &name })).await;
        assert_eq!(
            status, 200,
            "case GetIndex({case}, after the create): expected 200, got {status} {code} with {read}"
        );
        assert_eq!(
            read.get("indexSchema"),
            Some(&schema),
            "case GetIndex({case}, after the create): expected the stored schema {schema}, got {read}"
        );
    }
}
