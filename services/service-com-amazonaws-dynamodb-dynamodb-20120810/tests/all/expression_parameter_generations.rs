// SPDX-License-Identifier: CC0-1.0

//! A request picks ONE generation of parameters.
//!
//! Every reader and writer that still declares the non-expression (legacy) members —
//! `AttributesToGet`, `ScanFilter`, `QueryFilter`, `KeyConditions`, `AttributeUpdates`,
//! `Expected`, `ConditionalOperator` — refuses a request that carries ANY of them beside
//! ANY expression member of the same operation (`ProjectionExpression`,
//! `FilterExpression`, `KeyConditionExpression`, `UpdateExpression`,
//! `ConditionExpression`), and the sentence it is refused with names the WHOLE of each
//! side the request carried.
//!
//! What is held against a request is the ASK it states twice, read over the two
//! generations: the expression language supersedes the members that NARROW a request — a
//! projection, a filter, the operator that combines a legacy map's entries — so any
//! expression member beside one of them is refused, naming the whole of each side. The
//! table below is therefore keyed on the CROSS combinations — every superseded legacy
//! member of an operation against every expression member it declares that did NOT replace
//! it — plus the cross-ENTRY form of it on `BatchGetItem`, whose members live one level
//! down in `RequestItems`.
//!
//! The one legacy member that is NOT narrowing is the one a `Query` states its key
//! condition through, which the operation cannot run without: it is resolved from whichever
//! generation carries it and is held only against the member that replaced it, so a `Query`
//! keying the legacy way and filtering or projecting the expression way is SERVED — the
//! second table below drives those rows.
//!
//! The members each side lists, and the order they are listed in, are the ones
//! `smithy_query(query="operation", operation="<Scan|Query|GetItem|PutItem|UpdateItem\
//! |DeleteItem|BatchGetItem>")` declares for that operation's input shape: the expression
//! side names every expression member the request carried, and the legacy side names every
//! superseded member, by the ask each states.
//! `tests/all/query_mixed_parameter_order.rs` walks that ordering over every subset of both
//! sides.
//!
//! The last table pins the other side of the rule: a request that stays inside one
//! generation — either one — is served, so what the refusal is about is the MIXING and
//! not the legacy members themselves.
//!
//! A second rule follows the generations too: a VALUE is held to the documented value
//! rules wherever a request carries one, so a value the legacy `Expected` map compares
//! against earns exactly the clause the same value earns inside an item or bound by an
//! expression. Everything is driven over the wire.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table every row drives: a composite key, so a `Query` can state a key condition,
/// plus the non-key attributes the projections and filters name.
const TABLE: &str = "ParameterGenerations";

/// A second table, so a `BatchGetItem` can address two entries at once and state each
/// one's projection in a different generation.
const OTHER_TABLE: &str = "ParameterGenerationsOther";

/// The sentence a mixed request earns, with both sides filled in.
///
/// `enveloped` is whether it is stated inside the `<N> validation error(s) detected: `
/// envelope, which follows the MEMBERS the two sides name: a contradiction naming a member
/// a WRITE states its intent through carries the envelope, and one between the members a
/// READ narrows its result with is stated on its own.
fn mixed(legacy: &str, expression: &str, enveloped: bool) -> String {
    let envelope = if enveloped {
        "1 validation error detected: "
    } else {
        ""
    };
    format!(
        "{envelope}Can not use both expression and non-expression parameters in the same \
         request: Non-expression parameters: {{{legacy}}} Expression parameters: \
         {{{expression}}}"
    )
}

/// An emulator holding one active table and one item, so a served row is served on real
/// state rather than on an empty table.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the parameter-generations table", TABLE).await;
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": OTHER_TABLE,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the second parameter-generations table", OTHER_TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": TABLE,
            "Item": {"pk": {"S": "k1"}, "sk": {"S": "s1"}, "a": {"S": "x"}, "b": {"S": "y"}},
        }),
    )
    .await;
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": OTHER_TABLE,
            "Item": {"pk": {"S": "k1"}, "a": {"S": "x"}, "b": {"S": "y"}},
        }),
    )
    .await;
    router
}

#[tokio::test]
async fn a_legacy_member_beside_any_expression_member_is_refused() {
    let router = seeded().await;

    // Each row: the case, the operation, the request, and the two sides the sentence must
    // name. The rows span the readers and the writers, which legacy member is carried,
    // which expression member it is carried beside — never the one that replaced it — and
    // how many members each side holds.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a Scan projecting the legacy way and filtering the expression way",
            "Scan",
            json!({
                "TableName": TABLE,
                "AttributesToGet": ["a"],
                "FilterExpression": "attribute_exists(a)",
            }),
            "AttributesToGet",
            "FilterExpression",
        ),
        (
            "a Scan filtering the legacy way and projecting the expression way",
            "Scan",
            json!({
                "TableName": TABLE,
                "ScanFilter": {"a": {"ComparisonOperator": "NOT_NULL"}},
                "ProjectionExpression": "a",
            }),
            "ScanFilter",
            "ProjectionExpression",
        ),
        (
            "a Scan carrying the legacy operator beside a projection expression",
            "Scan",
            json!({
                "TableName": TABLE,
                "ConditionalOperator": "AND",
                "ProjectionExpression": "a",
            }),
            "ConditionalOperator",
            "ProjectionExpression",
        ),
        (
            "a Scan carrying both legacy projection members beside one expression member",
            "Scan",
            json!({
                "TableName": TABLE,
                "AttributesToGet": ["a"],
                "ScanFilter": {
                    "a": {"ComparisonOperator": "NOT_NULL"},
                    "b": {"ComparisonOperator": "NOT_NULL"},
                },
                "ConditionalOperator": "AND",
                "ProjectionExpression": "a",
            }),
            "AttributesToGet, ScanFilter, ConditionalOperator",
            "ProjectionExpression",
        ),
        (
            "a Query projecting the legacy way and keying the expression way",
            "Query",
            json!({
                "TableName": TABLE,
                "AttributesToGet": ["a"],
                "KeyConditionExpression": "pk = :k",
                "ExpressionAttributeValues": {":k": {"S": "k1"}},
            }),
            "AttributesToGet",
            "KeyConditionExpression",
        ),
        (
            "a Query keying the legacy way and projecting the expression way",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "KeyConditionExpression": "pk = :k",
                "ProjectionExpression": "a",
                "ExpressionAttributeValues": {":k": {"S": "k1"}},
            }),
            // The key condition is the one legacy member a request may state beside an
            // expression member of another ask, so this row states it in BOTH generations
            // to be refused at all — the served form of it is in the table below.
            "KeyConditions",
            "ProjectionExpression, KeyConditionExpression",
        ),
        (
            "a Query keying both ways and filtering the expression way",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "KeyConditionExpression": "pk = :k",
                "FilterExpression": "attribute_exists(a)",
                "ExpressionAttributeValues": {":k": {"S": "k1"}},
            }),
            "KeyConditions",
            "FilterExpression, KeyConditionExpression",
        ),
        (
            "a Query filtering the legacy way and keying the expression way",
            "Query",
            json!({
                "TableName": TABLE,
                "QueryFilter": {"a": {"ComparisonOperator": "NOT_NULL"}},
                "KeyConditionExpression": "pk = :k",
                "ExpressionAttributeValues": {":k": {"S": "k1"}},
            }),
            "QueryFilter",
            "KeyConditionExpression",
        ),
        (
            "a Query carrying two legacy members beside two expression members",
            "Query",
            json!({
                "TableName": TABLE,
                "AttributesToGet": ["a"],
                "QueryFilter": {"a": {"ComparisonOperator": "NOT_NULL"}},
                "FilterExpression": "attribute_exists(b)",
                "KeyConditionExpression": "pk = :k",
                "ExpressionAttributeValues": {":k": {"S": "k1"}},
            }),
            "AttributesToGet, QueryFilter",
            "FilterExpression, KeyConditionExpression",
        ),
        (
            "an UpdateItem expecting the legacy way and updating the expression way",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "Expected": {"c": {"Exists": false}},
                "UpdateExpression": "SET c = :z",
                "ExpressionAttributeValues": {":z": {"S": "z"}},
            }),
            "Expected",
            "UpdateExpression",
        ),
        (
            "an UpdateItem updating the legacy way and conditioning the expression way",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "AttributeUpdates": {"c": {"Action": "PUT", "Value": {"S": "z"}}},
                "ConditionExpression": "attribute_exists(pk)",
            }),
            "AttributeUpdates",
            "ConditionExpression",
        ),
        (
            "an UpdateItem carrying the legacy operator beside an update expression",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "ConditionalOperator": "AND",
                "UpdateExpression": "SET c = :z",
                "ExpressionAttributeValues": {":z": {"S": "z"}},
            }),
            "ConditionalOperator",
            "UpdateExpression",
        ),
        (
            "a GetItem projecting both ways",
            "GetItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "AttributesToGet": ["a"],
                "ProjectionExpression": "a",
            }),
            "AttributesToGet",
            "ProjectionExpression",
        ),
        (
            "a PutItem conditioning both ways",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k2"}, "sk": {"S": "s2"}},
                "Expected": {"a": {"Exists": false}},
                "ConditionExpression": "attribute_not_exists(a)",
            }),
            "Expected",
            "ConditionExpression",
        ),
        (
            "a DeleteItem carrying the legacy operator beside a condition expression",
            "DeleteItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "ConditionalOperator": "AND",
                "ConditionExpression": "attribute_exists(pk)",
            }),
            "ConditionalOperator",
            "ConditionExpression",
        ),
        // The rule is about the REQUEST, so a batch states its generation once across
        // every entry: one entry projecting the legacy way and another the expression way
        // mixes them exactly as one entry carrying both members does.
        (
            "a BatchGetItem whose entries project in different generations",
            "BatchGetItem",
            json!({
                "RequestItems": {
                    TABLE: {
                        "Keys": [{"pk": {"S": "k1"}, "sk": {"S": "s1"}}],
                        "AttributesToGet": ["a"],
                    },
                    OTHER_TABLE: {
                        "Keys": [{"pk": {"S": "k1"}}],
                        "ProjectionExpression": "b",
                    },
                },
            }),
            "AttributesToGet",
            "ProjectionExpression",
        ),
    ];

    for (case, operation, request, legacy, expression) in rows {
        // The envelope follows the MEMBERS the two sides name: every expression member the
        // three item writers declare — `UpdateExpression` and `ConditionExpression` — is
        // one a WRITE states its intent through, and a mixed request always names one of
        // them, so those surfaces carry the envelope while the readers state the sentence
        // alone.
        let enveloped = matches!(operation, "PutItem" | "UpdateItem" | "DeleteItem");
        let expected = mixed(legacy, expression, enveloped);
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected {expected:?}, got {status} with {answered}"
        );
    }
}

#[tokio::test]
async fn a_query_keying_the_legacy_way_may_state_another_ask_the_expression_way() {
    let router = seeded().await;

    // The key condition is the one legacy member a request may keep beside the expression
    // language: a `Query` cannot run without one ("Either the KeyConditions or
    // KeyConditionExpression parameter must be specified in the request"), so the reader
    // takes it from whichever generation carries it and refuses it only beside the member
    // that replaced it. The rows vary which OTHER ask is stated the expression way \u2014 the
    // projection, the filter, both \u2014 and each answer shows that ask was honoured.
    for (case, request, items) in [
        (
            "keying the legacy way and projecting the expression way",
            json!({
                "TableName": TABLE,
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "ProjectionExpression": "a",
            }),
            json!([{"a": {"S": "x"}}]),
        ),
        (
            "keying the legacy way and filtering the expression way",
            json!({
                "TableName": TABLE,
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "FilterExpression": "attribute_not_exists(a)",
            }),
            json!([]),
        ),
        (
            "keying the legacy way, filtering and projecting the expression way",
            json!({
                "TableName": TABLE,
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "FilterExpression": "a = :v",
                "ProjectionExpression": "b",
                "ExpressionAttributeValues": {":v": {"S": "x"}},
            }),
            json!([{"b": {"S": "y"}}]),
        ),
    ] {
        let served = ok(&router, "Query", request).await;
        assert_eq!(
            served["Items"], items,
            "case {case:?}: the expression ask must be honoured beside the legacy key \
             condition, got {served}"
        );
    }
}

#[tokio::test]
async fn a_request_inside_one_generation_is_served() {
    let router = seeded().await;

    // Each row: the case and a request whose members all belong to ONE generation,
    // spanning both generations on each reader and writer — so the refusal above is the
    // mixing being refused and not either generation's members.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a Scan filtering and projecting the legacy way",
            "Scan",
            json!({
                "TableName": TABLE,
                "AttributesToGet": ["a"],
                "ScanFilter": {
                    "a": {"ComparisonOperator": "NOT_NULL"},
                    "b": {"ComparisonOperator": "NOT_NULL"},
                },
                "ConditionalOperator": "AND",
            }),
        ),
        (
            "a Scan filtering and projecting the expression way",
            "Scan",
            json!({
                "TableName": TABLE,
                "ProjectionExpression": "a",
                "FilterExpression": "attribute_exists(a)",
            }),
        ),
        (
            "a Query keying, filtering and projecting the legacy way",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "QueryFilter": {"a": {"ComparisonOperator": "NOT_NULL"}},
                "AttributesToGet": ["a"],
            }),
        ),
        (
            "a Query keying, filtering and projecting the expression way",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditionExpression": "pk = :k",
                "FilterExpression": "attribute_exists(a)",
                "ProjectionExpression": "a",
                "ExpressionAttributeValues": {":k": {"S": "k1"}},
            }),
        ),
        (
            "a GetItem projecting the legacy way",
            "GetItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "AttributesToGet": ["a"],
            }),
        ),
        (
            "an UpdateItem expecting and updating the legacy way",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "Expected": {"c": {"Exists": false}},
                "AttributeUpdates": {"c": {"Action": "PUT", "Value": {"S": "z"}}},
            }),
        ),
        (
            "an UpdateItem conditioning and updating the expression way",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "ConditionExpression": "attribute_exists(pk)",
                "UpdateExpression": "SET d = :z",
                "ExpressionAttributeValues": {":z": {"S": "z"}},
            }),
        ),
        (
            "a BatchGetItem whose entries both project the legacy way",
            "BatchGetItem",
            json!({
                "RequestItems": {
                    TABLE: {
                        "Keys": [{"pk": {"S": "k1"}, "sk": {"S": "s1"}}],
                        "AttributesToGet": ["a"],
                    },
                    OTHER_TABLE: {
                        "Keys": [{"pk": {"S": "k1"}}],
                        "AttributesToGet": ["b"],
                    },
                },
            }),
        ),
        (
            "a BatchGetItem whose entries both project the expression way",
            "BatchGetItem",
            json!({
                "RequestItems": {
                    TABLE: {
                        "Keys": [{"pk": {"S": "k1"}, "sk": {"S": "s1"}}],
                        "ProjectionExpression": "a",
                    },
                    OTHER_TABLE: {
                        "Keys": [{"pk": {"S": "k1"}}],
                        "ProjectionExpression": "b",
                    },
                },
            }),
        ),
    ];

    for (case, operation, request) in rows {
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a request inside one generation is served, got {status} with \
             {answered}"
        );
    }
}

/// The documented maximum depth a value may nest to: "DynamoDB supports nested attributes
/// up to 32 levels deep" ("Service, Account, and Table Quotas"), so a value nesting deeper
/// is outside what any surface admits.
const MAX_ATTRIBUTE_DEPTH: usize = 32;

/// The clause a value nesting past that maximum earns, inside the envelope an item-API
/// member's clause is stated in.
const DEEP_VALUE_MESSAGE: &str = "1 validation error detected: Nesting Levels have exceeded \
                                  supported limits: Attributes in the item have nested levels \
                                  beyond supported limit";

/// The clause an empty string set earns, whichever member carried it.
const EMPTY_SET_MESSAGE: &str = "1 validation error detected: One or more parameter values were \
                                 invalid: An string set  may not be empty";

/// An attribute value nesting `levels` containers deep, each level a one-element list.
fn nested(levels: usize) -> Value {
    let mut value = json!({"S": "deep"});
    for _ in 0..levels {
        value = json!({"L": [value]});
    }
    value
}

#[tokio::test]
async fn a_value_the_legacy_generation_carries_is_held_to_the_same_rules() {
    let router = seeded().await;

    // Each row: the case, the operation, the request and the clause it earns. The rows
    // carry the SAME offending value through both generations of every writer that
    // declares the legacy `Expected` map — the entry's `Value`, an element of its
    // `AttributeValueList`, and the expression generation's `ExpressionAttributeValues` —
    // and vary the rule broken (the documented nesting depth, and a set type's own rule).
    let deep = nested(MAX_ATTRIBUTE_DEPTH + 1);
    let empty_set = json!({"SS": []});
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a PutItem whose Expected entry compares against an over-deep value",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k9"}, "sk": {"S": "s9"}},
                "Expected": {"a": {"Value": deep.clone()}},
            }),
            DEEP_VALUE_MESSAGE,
        ),
        (
            "a PutItem whose Expected entry lists an over-deep operand",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k9"}, "sk": {"S": "s9"}},
                "Expected": {"a": {"ComparisonOperator": "EQ",
                                   "AttributeValueList": [deep.clone()]}},
            }),
            DEEP_VALUE_MESSAGE,
        ),
        (
            "a PutItem binding an over-deep value through its expression",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k9"}, "sk": {"S": "s9"}},
                "ConditionExpression": "a = :v",
                "ExpressionAttributeValues": {":v": deep.clone()},
            }),
            DEEP_VALUE_MESSAGE,
        ),
        (
            "an UpdateItem whose Expected entry compares against an over-deep value",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "AttributeUpdates": {"c": {"Action": "PUT", "Value": {"S": "z"}}},
                "Expected": {"a": {"Value": deep.clone()}},
            }),
            DEEP_VALUE_MESSAGE,
        ),
        (
            "a DeleteItem whose Expected entry compares against an over-deep value",
            "DeleteItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "Expected": {"a": {"Value": deep.clone()}},
            }),
            DEEP_VALUE_MESSAGE,
        ),
        (
            "a DeleteItem whose Expected entry compares against an empty set",
            "DeleteItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "Expected": {"a": {"Value": empty_set.clone()}},
            }),
            EMPTY_SET_MESSAGE,
        ),
        (
            "a DeleteItem binding an empty set through its expression",
            "DeleteItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "ConditionExpression": "a = :v",
                "ExpressionAttributeValues": {":v": empty_set.clone()},
            }),
            EMPTY_SET_MESSAGE,
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.to_string()),
            "case {case:?}: expected {expected:?}, got {status} with {answered}"
        );
    }

    // The fixture's item is untouched by every refusal above: a request refused for the
    // value it carried never reached the store.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["a"],
        json!({"S": "x"}),
        "the seeded item survives every refusal, got {read}"
    );
    assert_eq!(
        read["Item"]["c"],
        Value::Null,
        "no refused update wrote its attribute, got {read}"
    );
}
