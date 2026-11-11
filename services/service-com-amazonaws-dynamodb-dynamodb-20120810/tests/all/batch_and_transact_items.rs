// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MULTI-ITEM paths — `BatchWriteItem`, `BatchGetItem`,
//! `TransactWriteItems` and `TransactGetItems` — held to the rule every write path of
//! this service shares: a write a 200 acknowledges is IN THE STORE, so every later read
//! reports exactly what the last write left, and a write that reports an item reports it
//! as it stood immediately before that write.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, so every
//! assertion is about what a LATER request reports.
//!
//! Each suite is a TABLE spanning the class the rule generalises over: which path the
//! write took (single item, batch, transaction), which reader reads it back (`GetItem`,
//! `BatchGetItem`, `TransactGetItems`, `Query`), the attribute types the item carries and
//! the key schema its table declares.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use smithy_server_runtime::MultiServiceRouter;

// One harness for the whole target: the router, the request and the error-code reader
// come from `tests/all/emulator.rs`; the table factory from `tests/all/table_items.rs`.
use crate::emulator::{call, call_in, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// Send one operation in `region` that must succeed, for the cases that are ABOUT the
/// region a request is signed for.
async fn ok_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call_in(router, region, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {operation} in {region} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// One item: the key's attributes plus `extra`.
fn item_with(key: &Value, extra: &Value) -> Value {
    let mut merged = key.clone();
    for (name, value) in extra.as_object().expect("an object of attributes") {
        merged[name] = value.clone();
    }
    merged
}

/// The item stored under `key` as `GetItem` reports it, or `Null` when the key holds
/// none.
async fn stored(router: &MultiServiceRouter, table: &str, key: &Value) -> Value {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table, "Key": key, "ConsistentRead": true}),
    )
    .await;
    read.get("Item").cloned().unwrap_or(Value::Null)
}

/// The item under `key` as EVERY read path reports it, named by the path: `GetItem`,
/// `BatchGetItem` and `TransactGetItems` all report the one stored item, so a write is
/// only persisted if all of them agree.
async fn stored_everywhere(
    router: &MultiServiceRouter,
    table: &str,
    key: &Value,
) -> Vec<(&'static str, Value)> {
    let batch = ok(
        router,
        "BatchGetItem",
        json!({"RequestItems": {table: {"Keys": [key], "ConsistentRead": true}}}),
    )
    .await;
    let from_batch = batch["Responses"][table]
        .as_array()
        .and_then(|items| items.first().cloned())
        .unwrap_or(Value::Null);
    let transactional = ok(
        router,
        "TransactGetItems",
        json!({"TransactItems": [{"Get": {"TableName": table, "Key": key}}]}),
    )
    .await;
    let from_transaction = transactional["Responses"][0]
        .get("Item")
        .cloned()
        .unwrap_or(Value::Null);
    vec![
        ("GetItem", stored(router, table, key).await),
        ("BatchGetItem", from_batch),
        ("TransactGetItems", from_transaction),
    ]
}

/// A `BatchWriteItem` that puts `item`.
fn batch_put(table: &str, item: &Value) -> Value {
    json!({"RequestItems": {table: [{"PutRequest": {"Item": item}}]}})
}

/// A `TransactWriteItems` that puts `item`.
fn transact_put(table: &str, item: &Value) -> Value {
    json!({"TransactItems": [{"Put": {"TableName": table, "Item": item}}]})
}

/// One row of the write-path table: the operation that writes an item, and the operation
/// that removes it again, each built from the table name and the item (or its key).
struct WritePath {
    case: &'static str,
    write: fn(&str, &Value) -> Value,
    write_operation: &'static str,
    remove: fn(&str, &Value) -> Value,
    remove_operation: &'static str,
}

/// Every path that stores a whole item, and the path that removes it again: the rule is
/// about a write landing in the store, so each of them must land in the SAME store the
/// others read.
fn write_paths() -> Vec<WritePath> {
    vec![
        WritePath {
            case: "PutItem / DeleteItem",
            write: |table, item| json!({"TableName": table, "Item": item}),
            write_operation: "PutItem",
            remove: |table, key| json!({"TableName": table, "Key": key}),
            remove_operation: "DeleteItem",
        },
        WritePath {
            case: "BatchWriteItem PutRequest / DeleteRequest",
            write: batch_put,
            write_operation: "BatchWriteItem",
            remove: |table, key| {
                json!({"RequestItems": {table: [{"DeleteRequest": {"Key": key}}]}})
            },
            remove_operation: "BatchWriteItem",
        },
        WritePath {
            case: "TransactWriteItems Put / Delete",
            write: transact_put,
            write_operation: "TransactWriteItems",
            remove: |table, key| {
                json!({"TransactItems": [{"Delete": {"TableName": table, "Key": key}}]})
            },
            remove_operation: "TransactWriteItems",
        },
    ]
}

/// A write acknowledged with 200 is in the store on EVERY write path, and every read
/// path agrees about it: the item reads back exactly as written, a second write of a
/// smaller item REPLACES it whole, and the remove leaves no item behind.
///
/// The rows span the three paths that store an item and, for each, an item carrying
/// every `AttributeValue` kind a row can differ along.
#[tokio::test]
async fn every_write_path_lands_in_the_store_every_reader_reads() {
    let router = emulator();
    let key = json!({"pk": {"S": "k1"}, "sk": {"N": "7"}});
    for (index, path) in write_paths().into_iter().enumerate() {
        let table = format!("paths-{index}");
        create_active_table(
            &router,
            &table,
            &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
        )
        .await;

        let full = item_with(
            &key,
            &json!({
                "ver": {"N": "1"},
                "tags": {"SS": ["x", "y"]},
                "meta": {"M": {"a": {"S": "1"}}},
                "blob": {"B": "AAH+/w=="},
                "flag": {"BOOL": true},
                "list": {"L": [{"N": "1"}, {"S": "two"}]},
            }),
        );
        ok(&router, path.write_operation, (path.write)(&table, &full)).await;
        for (reader, read) in stored_everywhere(&router, &table, &key).await {
            assert_eq!(
                read, full,
                "case {} read by {reader}: the written item reads back exactly as written, \
                 got {read}",
                path.case
            );
        }

        // "The new item completely replaces the existing item": the attributes the
        // second write omits are gone rather than merged into what was there.
        let smaller = item_with(&key, &json!({"ver": {"N": "2"}}));
        ok(&router, path.write_operation, (path.write)(&table, &smaller)).await;
        for (reader, read) in stored_everywhere(&router, &table, &key).await {
            assert_eq!(
                read, smaller,
                "case {} read by {reader}: a write of an item REPLACES the whole item, \
                 got {read}",
                path.case
            );
        }

        ok(&router, path.remove_operation, (path.remove)(&table, &key)).await;
        for (reader, read) in stored_everywhere(&router, &table, &key).await {
            assert_eq!(
                read,
                Value::Null,
                "case {} read by {reader}: the removed item is gone, got {read}",
                path.case
            );
        }
    }
}

/// One `BatchWriteItem` spanning several tables and both kinds of request applies every
/// one of them: the puts land, the deletes remove, and nothing else in the tables moves.
///
/// The rows differ along the COUNT and the mix the batch carries, which is what the rule
/// generalises over.
#[tokio::test]
async fn one_batch_applies_every_put_and_delete_it_carries() {
    let router = emulator();
    let first = "batch-mixed-a";
    let second = "batch-mixed-b";
    create_active_table(&router, first, &[("pk", "HASH", "S")]).await;
    create_active_table(&router, second, &[("pk", "HASH", "N")]).await;

    // Three items stand before the batch; the batch deletes one of them, replaces
    // another, leaves the third alone and creates two more in the other table.
    for name in ["keep", "replace", "drop"] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": first, "Item": {"pk": {"S": name}, "ver": {"N": "1"}}}),
        )
        .await;
    }

    let applied = ok(
        &router,
        "BatchWriteItem",
        json!({
            "RequestItems": {
                first: [
                    {"PutRequest": {"Item": {"pk": {"S": "replace"}, "ver": {"N": "2"}}}},
                    {"DeleteRequest": {"Key": {"pk": {"S": "drop"}}}},
                ],
                second: [
                    {"PutRequest": {"Item": {"pk": {"N": "1"}, "note": {"S": "one"}}}},
                    {"PutRequest": {"Item": {"pk": {"N": "2"}, "note": {"S": "two"}}}},
                ],
            },
            "ReturnConsumedCapacity": "TOTAL",
        }),
    )
    .await;
    // "A map of tables and requests against those tables that were not processed":
    // everything was processed, so the map is empty — never absent.
    assert_eq!(
        applied["UnprocessedItems"],
        json!({}),
        "case mixed batch: every accepted operation is processed, got {applied}"
    );
    let charged: Vec<&str> = applied["ConsumedCapacity"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["TableName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        charged.contains(&first) && charged.contains(&second),
        "case mixed batch: each table written reports its own capacity, got {applied}"
    );

    let expected: Vec<(&str, Value, Value)> = vec![
        (
            "untouched item stays",
            json!({"TableName": first, "Key": {"pk": {"S": "keep"}}}),
            json!({"pk": {"S": "keep"}, "ver": {"N": "1"}}),
        ),
        (
            "put replaces in place",
            json!({"TableName": first, "Key": {"pk": {"S": "replace"}}}),
            json!({"pk": {"S": "replace"}, "ver": {"N": "2"}}),
        ),
        (
            "delete removes",
            json!({"TableName": first, "Key": {"pk": {"S": "drop"}}}),
            Value::Null,
        ),
        (
            "put creates in the other table",
            json!({"TableName": second, "Key": {"pk": {"N": "1"}}}),
            json!({"pk": {"N": "1"}, "note": {"S": "one"}}),
        ),
        (
            "second put creates in the other table",
            json!({"TableName": second, "Key": {"pk": {"N": "2"}}}),
            json!({"pk": {"N": "2"}, "note": {"S": "two"}}),
        ),
    ];
    for (case, request, want) in expected {
        let read = ok(&router, "GetItem", request.clone()).await;
        let item = read.get("Item").cloned().unwrap_or(Value::Null);
        assert_eq!(
            item, want,
            "case {case}: the batch left exactly this item at {request}, got {item}"
        );
    }
}

/// `BatchGetItem` reports the stored item behind each key and nothing for a key that
/// holds none — and the rows span how many of a table's keys are there.
#[tokio::test]
async fn batch_get_item_reports_only_the_keys_that_hold_items() {
    let router = emulator();
    let table = "batch-get";
    create_active_table(&router, table, &[("pk", "HASH", "S")]).await;
    for name in ["a", "b"] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": {"pk": {"S": name}, "ver": {"N": "1"}}}),
        )
        .await;
    }

    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("every key present", vec!["a", "b"], vec!["a", "b"]),
        ("one key absent", vec!["a", "absent"], vec!["a"]),
        ("every key absent", vec!["absent", "missing"], vec![]),
    ];
    for (case, keys, want) in cases {
        let requested: Vec<Value> = keys.iter().map(|name| json!({"pk": {"S": name}})).collect();
        let read = ok(
            &router,
            "BatchGetItem",
            json!({
                "RequestItems": {table: {"Keys": requested, "ConsistentRead": true}},
                "ReturnConsumedCapacity": "TOTAL",
            }),
        )
        .await;
        let mut reported: Vec<String> = read["Responses"][table]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item["pk"]["S"].as_str().map(ToString::to_string))
            .collect();
        reported.sort();
        assert_eq!(
            reported,
            want.iter().map(ToString::to_string).collect::<Vec<String>>(),
            "case {case}: only the keys that hold an item are reported, got {read}"
        );
        assert_eq!(
            read["UnprocessedKeys"],
            json!({}),
            "case {case}: every accepted key was read, got {read}"
        );
        assert!(
            read["ConsumedCapacity"][0]["CapacityUnits"].as_f64().unwrap_or(0.0) > 0.0,
            "case {case}: the read it performed is charged, got {read}"
        );
    }
}

/// A transaction is all-or-nothing: when one action's condition is not met NOTHING is
/// written, the call is `TransactionCanceledException`, and its `CancellationReasons`
/// carry one reason per action in request order — the failing one naming its code (and
/// its item when it asked for it), the rest `None`.
///
/// The rows span WHICH action of the transaction fails, since the rule is about the
/// transaction and not about one position in it.
#[tokio::test]
async fn an_unmet_condition_cancels_the_whole_transaction() {
    let router = emulator();
    let table = "transact-cancel";
    create_active_table(&router, table, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": table, "Item": {"pk": {"S": "stands"}, "ver": {"N": "1"}}}),
    )
    .await;

    // The action that cannot hold: `stands` exists, so `attribute_not_exists` on it is
    // false whatever position it is written in.
    let doomed = json!({
        "Put": {
            "TableName": table,
            "Item": {"pk": {"S": "stands"}, "ver": {"N": "99"}},
            "ConditionExpression": "attribute_not_exists(pk)",
            "ReturnValuesOnConditionCheckFailure": "ALL_OLD",
        }
    });
    let innocent = json!({
        "Put": {"TableName": table, "Item": {"pk": {"S": "fresh"}, "ver": {"N": "1"}}}
    });
    let cases: Vec<(&str, Vec<Value>, usize)> = vec![
        (
            "failing action first",
            vec![doomed.clone(), innocent.clone()],
            0,
        ),
        (
            "failing action last",
            vec![innocent.clone(), doomed.clone()],
            1,
        ),
        (
            "failing action in the middle",
            vec![
                innocent.clone(),
                doomed.clone(),
                json!({"ConditionCheck": {
                    "TableName": table,
                    "Key": {"pk": {"S": "elsewhere"}},
                    "ConditionExpression": "attribute_not_exists(pk)",
                }}),
            ],
            1,
        ),
    ];
    for (case, actions, failing) in cases {
        let count = actions.len();
        let (status, body) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": actions}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a cancelled transaction is refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "TransactionCanceledException",
            "case {case}: the refusal names the cancellation, got {body}"
        );
        let reasons = body["CancellationReasons"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reasons.len(),
            count,
            "case {case}: one reason per action, got {body}"
        );
        for (at, reason) in reasons.iter().enumerate() {
            let want = if at == failing {
                "ConditionalCheckFailed"
            } else {
                "None"
            };
            assert_eq!(
                reason["Code"],
                json!(want),
                "case {case} reason {at}: expected code {want}, got {reason}"
            );
        }
        assert_eq!(
            reasons[failing]["Item"],
            json!({"pk": {"S": "stands"}, "ver": {"N": "1"}}),
            "case {case}: ALL_OLD on the failed check reports the item it read, got {body}"
        );

        // Nothing the transaction carried was written: the item it would have replaced
        // still stands, and the one it would have created is absent.
        let untouched = stored(&router, table, &json!({"pk": {"S": "stands"}})).await;
        assert_eq!(
            untouched,
            json!({"pk": {"S": "stands"}, "ver": {"N": "1"}}),
            "case {case}: the cancelled transaction wrote nothing, got {untouched}"
        );
        let absent = stored(&router, table, &json!({"pk": {"S": "fresh"}})).await;
        assert_eq!(
            absent,
            Value::Null,
            "case {case}: the cancelled transaction created nothing, got {absent}"
        );
    }
}

/// A transaction whose conditions all hold applies every action it carries — a put, an
/// update through an `UpdateExpression`, a delete and a condition check — and every read
/// path sees the result.
#[tokio::test]
async fn a_transaction_applies_every_action_it_carries() {
    let router = emulator();
    let table = "transact-apply";
    create_active_table(&router, table, &[("pk", "HASH", "S")]).await;
    for name in ["updated", "deleted", "checked"] {
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": table,
                "Item": {"pk": {"S": name}, "ver": {"N": "1"}, "via": {"S": "put"}},
            }),
        )
        .await;
    }

    ok(
        &router,
        "TransactWriteItems",
        json!({
            "TransactItems": [
                {"Put": {"TableName": table, "Item": {"pk": {"S": "created"}, "via": {"S": "transaction"}}}},
                {"Update": {
                    "TableName": table,
                    "Key": {"pk": {"S": "updated"}},
                    "UpdateExpression": "SET via = :via ADD ver :one",
                    "ExpressionAttributeValues": {":via": {"S": "updated"}, ":one": {"N": "4"}},
                }},
                {"Delete": {"TableName": table, "Key": {"pk": {"S": "deleted"}}}},
                {"ConditionCheck": {
                    "TableName": table,
                    "Key": {"pk": {"S": "checked"}},
                    "ConditionExpression": "ver = :one",
                    "ExpressionAttributeValues": {":one": {"N": "1"}},
                }},
            ],
            "ReturnConsumedCapacity": "TOTAL",
        }),
    )
    .await;

    let expected: Vec<(&str, Value, Value)> = vec![
        (
            "put creates",
            json!({"pk": {"S": "created"}}),
            json!({"pk": {"S": "created"}, "via": {"S": "transaction"}}),
        ),
        (
            "update applies its expression",
            json!({"pk": {"S": "updated"}}),
            json!({"pk": {"S": "updated"}, "ver": {"N": "5"}, "via": {"S": "updated"}}),
        ),
        (
            "delete removes",
            json!({"pk": {"S": "deleted"}}),
            Value::Null,
        ),
        (
            "a condition check writes nothing",
            json!({"pk": {"S": "checked"}}),
            json!({"pk": {"S": "checked"}, "ver": {"N": "1"}, "via": {"S": "put"}}),
        ),
    ];
    for (case, key, want) in expected {
        for (reader, read) in stored_everywhere(&router, table, &key).await {
            assert_eq!(
                read, want,
                "case {case} read by {reader}: expected {want}, got {read}"
            );
        }
    }

    // The update is visible to `Query` too, which reads the same stored items.
    let queried = ok(
        &router,
        "Query",
        json!({
            "TableName": table,
            "KeyConditionExpression": "pk = :pk",
            "ExpressionAttributeValues": {":pk": {"S": "updated"}},
        }),
    )
    .await;
    assert_eq!(
        queried["Count"],
        json!(1),
        "case query after a transactional update: the updated item is found, got {queried}"
    );
    assert_eq!(
        queried["Items"][0]["via"],
        json!({"S": "updated"}),
        "case query after a transactional update: the stored item carries the update, got \
         {queried}"
    );
}

/// "Providing a ClientRequestToken makes the call to TransactWriteItems idempotent,
/// meaning that multiple identical calls have the same effect as one single call": the
/// repeat applies nothing, and the same token carrying DIFFERENT actions is the
/// documented mismatch. The rows span the actions a repeat can carry.
#[tokio::test]
async fn a_client_request_token_makes_a_transaction_idempotent() {
    let router = emulator();
    let table = "transact-idempotent";
    create_active_table(&router, table, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": table, "Item": {"pk": {"S": "counted"}, "ver": {"N": "1"}}}),
    )
    .await;

    // An ADD is the action a re-application would be VISIBLE in, which is why the rule
    // is asserted with one.
    let transaction = json!({
        "ClientRequestToken": "token-one",
        "TransactItems": [{"Update": {
            "TableName": table,
            "Key": {"pk": {"S": "counted"}},
            "UpdateExpression": "ADD ver :one",
            "ExpressionAttributeValues": {":one": {"N": "1"}},
        }}],
    });
    for attempt in ["first call", "retry", "retry again"] {
        ok(&router, "TransactWriteItems", transaction.clone()).await;
        let read = stored(&router, table, &json!({"pk": {"S": "counted"}})).await;
        assert_eq!(
            read,
            json!({"pk": {"S": "counted"}, "ver": {"N": "2"}}),
            "case {attempt}: the transaction is applied exactly once, got {read}"
        );
    }

    // The same transaction re-spelled is still the same transaction: addressing the
    // table by its ARN, or spelling the same number differently, is a retry — the
    // fingerprint is taken over the RESOLVED table and the canonical values.
    let respelled: Vec<(&str, Value)> = vec![
        (
            "the table addressed by its ARN",
            json!({"ClientRequestToken": "token-one", "TransactItems": [{"Update": {
                "TableName": format!("arn:aws:dynamodb:us-east-1:000000000000:table/{table}"),
                "Key": {"pk": {"S": "counted"}},
                "UpdateExpression": "ADD ver :one",
                "ExpressionAttributeValues": {":one": {"N": "1"}},
            }}]}),
        ),
        (
            "the same number spelled differently",
            json!({"ClientRequestToken": "token-one", "TransactItems": [{"Update": {
                "TableName": table,
                "Key": {"pk": {"S": "counted"}},
                "UpdateExpression": "ADD ver :one",
                "ExpressionAttributeValues": {":one": {"N": "1.0"}},
            }}]}),
        ),
    ];
    for (case, body) in respelled {
        ok(&router, "TransactWriteItems", body).await;
        let read = stored(&router, table, &json!({"pk": {"S": "counted"}})).await;
        assert_eq!(
            read,
            json!({"pk": {"S": "counted"}, "ver": {"N": "2"}}),
            "case {case}: the retry is answered, not applied, got {read}"
        );
    }

    // "If you submit a request with the same client token but a change in other
    // parameters within the 10-minute idempotency window, DynamoDB returns an
    // IdempotentParameterMismatch exception."
    let changed: Vec<(&str, Value)> = vec![
        (
            "a different update",
            json!({"ClientRequestToken": "token-one", "TransactItems": [{"Update": {
                "TableName": table,
                "Key": {"pk": {"S": "counted"}},
                "UpdateExpression": "ADD ver :two",
                "ExpressionAttributeValues": {":two": {"N": "2"}},
            }}]}),
        ),
        (
            "a different item",
            json!({"ClientRequestToken": "token-one", "TransactItems": [{"Update": {
                "TableName": table,
                "Key": {"pk": {"S": "other"}},
                "UpdateExpression": "ADD ver :one",
                "ExpressionAttributeValues": {":one": {"N": "1"}},
            }}]}),
        ),
        (
            "a different action",
            json!({"ClientRequestToken": "token-one", "TransactItems": [{"Delete": {
                "TableName": table,
                "Key": {"pk": {"S": "counted"}},
            }}]}),
        ),
    ];
    for (case, body) in changed {
        let (status, refused) = call(&router, "TransactWriteItems", body).await;
        assert_eq!(
            status, 400,
            "case {case}: a token reused for other parameters is refused, got {status} with \
             {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "IdempotentParameterMismatchException",
            "case {case}: the refusal names the mismatch, got {refused}"
        );
        let read = stored(&router, table, &json!({"pk": {"S": "counted"}})).await;
        assert_eq!(
            read,
            json!({"pk": {"S": "counted"}, "ver": {"N": "2"}}),
            "case {case}: the refused call wrote nothing, got {read}"
        );
    }

    // A DIFFERENT token is a different request: it applies.
    let mut again = transaction.clone();
    again["ClientRequestToken"] = json!("token-two");
    ok(&router, "TransactWriteItems", again).await;
    let read = stored(&router, table, &json!({"pk": {"S": "counted"}})).await;
    assert_eq!(
        read,
        json!({"pk": {"S": "counted"}, "ver": {"N": "3"}}),
        "case a second token: a new token applies its own transaction, got {read}"
    );

    // A table name resolves inside ONE region, so a token names the transaction it ran
    // there: the same token, with the same actions, in another region is its own
    // request and must apply there.
    let elsewhere = "us-west-2";
    ok_in(
        &router,
        elsewhere,
        "CreateTable",
        json!({
            "TableName": table,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    ok_in(
        &router,
        elsewhere,
        "PutItem",
        json!({"TableName": table, "Item": {"pk": {"S": "counted"}, "ver": {"N": "1"}}}),
    )
    .await;
    ok_in(&router, elsewhere, "TransactWriteItems", transaction).await;
    let read_elsewhere = ok_in(
        &router,
        elsewhere,
        "GetItem",
        json!({"TableName": table, "Key": {"pk": {"S": "counted"}}, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read_elsewhere["Item"],
        json!({"pk": {"S": "counted"}, "ver": {"N": "2"}}),
        "case the same token in another region: it applies there, got {read_elsewhere}"
    );
    let read_here = stored(&router, table, &json!({"pk": {"S": "counted"}})).await;
    assert_eq!(
        read_here,
        json!({"pk": {"S": "counted"}, "ver": {"N": "3"}}),
        "case the same token in another region: the first region's items do not move, got \
         {read_here}"
    );
}

/// The ways a multi-item request can be wrong, each refused with the documented code and
/// each leaving the store untouched. The rows span the rules: one operation per request
/// entry, one operation per key, the documented ceiling on how many a call may carry, a
/// key that is not the table's, and a table that is not there.
#[tokio::test]
async fn a_refused_multi_item_request_is_refused_and_writes_nothing() {
    let router = emulator();
    let table = "multi-refusals";
    create_active_table(&router, table, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": table, "Item": {"pk": {"S": "stands"}, "ver": {"N": "1"}}}),
    )
    .await;

    // The documented ceilings, read from the service's own constants so a row cannot
    // drift from the rule it asserts.
    let over_batch_limit: Vec<Value> = (0..=service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_BATCH_WRITE_REQUESTS)
        .map(|at| json!({"PutRequest": {"Item": {"pk": {"S": format!("b{at}")}}}}))
        .collect();
    let over_get_limit: Vec<Value> = (0..=service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_BATCH_GET_KEYS)
        .map(|at| json!({"pk": {"S": format!("g{at}")}}))
        .collect();

    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a WriteRequest carrying both a put and a delete",
            "BatchWriteItem",
            json!({"RequestItems": {table: [{
                "PutRequest": {"Item": {"pk": {"S": "both"}}},
                "DeleteRequest": {"Key": {"pk": {"S": "both"}}},
            }]}}),
            "ValidationException",
        ),
        (
            "a WriteRequest carrying neither",
            "BatchWriteItem",
            json!({"RequestItems": {table: [{}]}}),
            "ValidationException",
        ),
        (
            "one key addressed twice in a batch",
            "BatchWriteItem",
            json!({"RequestItems": {table: [
                {"PutRequest": {"Item": {"pk": {"S": "twice"}, "ver": {"N": "1"}}}},
                {"DeleteRequest": {"Key": {"pk": {"S": "twice"}}}},
            ]}}),
            "ValidationException",
        ),
        (
            "one past the documented batch-write ceiling",
            "BatchWriteItem",
            json!({"RequestItems": {table: over_batch_limit}}),
            "ValidationException",
        ),
        (
            "one past the documented batch-get ceiling",
            "BatchGetItem",
            json!({"RequestItems": {table: {"Keys": over_get_limit}}}),
            "ValidationException",
        ),
        (
            "a key that is not the table's",
            "BatchWriteItem",
            json!({"RequestItems": {table: [{"PutRequest": {"Item": {"other": {"S": "x"}}}}]}}),
            "ValidationException",
        ),
        (
            "a table that is not there",
            "BatchWriteItem",
            json!({"RequestItems": {"absent-table": [
                {"PutRequest": {"Item": {"pk": {"S": "x"}}}},
            ]}}),
            "ResourceNotFoundException",
        ),
        (
            "one item addressed twice in a transaction",
            "TransactWriteItems",
            json!({"TransactItems": [
                {"Put": {"TableName": table, "Item": {"pk": {"S": "twice"}}}},
                {"Delete": {"TableName": table, "Key": {"pk": {"S": "twice"}}}},
            ]}),
            "ValidationException",
        ),
        (
            "a transaction entry carrying two actions",
            "TransactWriteItems",
            json!({"TransactItems": [{
                "Put": {"TableName": table, "Item": {"pk": {"S": "two"}}},
                "Delete": {"TableName": table, "Key": {"pk": {"S": "two"}}},
            }]}),
            "ValidationException",
        ),
        (
            "a transaction entry carrying none",
            "TransactWriteItems",
            json!({"TransactItems": [{}]}),
            "ValidationException",
        ),
        (
            "a transactional read of one item twice",
            "TransactGetItems",
            json!({"TransactItems": [
                {"Get": {"TableName": table, "Key": {"pk": {"S": "stands"}}}},
                {"Get": {"TableName": table, "Key": {"pk": {"S": "stands"}}}},
            ]}),
            "ValidationException",
        ),
        (
            "one table addressed by both its name and its ARN",
            "BatchGetItem",
            json!({"RequestItems": {
                table: {"Keys": [{"pk": {"S": "stands"}}]},
                format!("arn:aws:dynamodb:us-east-1:000000000000:table/{table}"):
                    {"Keys": [{"pk": {"S": "other"}}]},
            }}),
            "ValidationException",
        ),
    ];
    for (case, operation, body, want) in cases {
        let (status, refused) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            want,
            "case {case}: expected {want}, got {refused}"
        );
    }

    // Not one of the refusals above wrote anything: the item that stood still stands,
    // and no key a refused request named holds an item.
    let untouched = stored(&router, table, &json!({"pk": {"S": "stands"}})).await;
    assert_eq!(
        untouched,
        json!({"pk": {"S": "stands"}, "ver": {"N": "1"}}),
        "case after every refusal: the stored item is unchanged, got {untouched}"
    );
    for name in ["both", "twice", "two", "b0", "g0"] {
        let read = stored(&router, table, &json!({"pk": {"S": name}})).await;
        assert_eq!(
            read,
            Value::Null,
            "case after every refusal: a refused request wrote no item at {name}, got {read}"
        );
    }
}

/// A multi-item read reports what its entry's projection names, and an entry that states
/// that projection BOTH ways — through `ProjectionExpression` and through the legacy
/// `AttributesToGet` it replaced — is refused as the contradiction it is, the same way
/// `GetItem` refuses it. What a served projection REPORTS is the subject of
/// `tests/all/index_and_projection_reads.rs`.
#[tokio::test]
async fn a_projection_stated_twice_is_refused() {
    let router = emulator();
    let table = "multi-projection";
    create_active_table(&router, table, &[("pk", "HASH", "S")]).await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "BatchGetItem stating one table's projection both ways",
            "BatchGetItem",
            json!({"RequestItems": {table: {
                "Keys": [{"pk": {"S": "a"}}],
                "ProjectionExpression": "ver",
                "AttributesToGet": ["ver"],
            }}}),
        ),
        (
            "GetItem stating its projection both ways",
            "GetItem",
            json!({"TableName": table, "Key": {"pk": {"S": "a"}},
                   "ProjectionExpression": "ver", "AttributesToGet": ["ver"]}),
        ),
    ];
    for (case, operation, body) in cases {
        let (status, refused) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: a request that states one thing twice is refused, got {status} \
             with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            message(&refused),
            "Can not use both expression and non-expression parameters in the same request: \
             Non-expression parameters: {AttributesToGet} Expression parameters: \
             {ProjectionExpression}",
            "case {case}: expected the two-members sentence, got {refused}"
        );
    }
}

/// An attribute value nested `layers` deep: `layers` lists wrapping one string, which
/// is `layers + 1` levels of nesting ("DynamoDB supports nested attributes up to 32
/// levels deep").
fn nested(layers: usize) -> Value {
    let mut value = json!({"S": "deep"});
    for _ in 0..layers {
        value = json!({"L": [value]});
    }
    value
}

/// A transactional write carries its item through the transaction's own canonical
/// encoding (what a repeated `ClientRequestToken` is compared against), so a deeply
/// nested item must be ENCODED without the walk's cost depending on how deep the
/// request chose to nest: an item at the documented limit is stored and reads back, and
/// one past it is refused. The rows are the documented bound and one level past it.
#[tokio::test]
async fn a_transaction_carries_nesting_up_to_the_documented_limit() {
    let router = emulator();
    let table = "transact-nesting";
    create_active_table(&router, table, &[("pk", "HASH", "S")]).await;
    let limit = service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_ATTRIBUTE_DEPTH;

    // At the limit: `limit - 1` lists around the scalar is `limit` levels.
    let deep = nested(limit - 1);
    let item = json!({"pk": {"S": "nested"}, "tree": deep});
    ok(
        &router,
        "TransactWriteItems",
        json!({
            "ClientRequestToken": "nesting-token",
            "TransactItems": [{"Put": {"TableName": table, "Item": item}}],
        }),
    )
    .await;
    let read = stored(&router, table, &json!({"pk": {"S": "nested"}})).await;
    assert_eq!(
        read, item,
        "case nesting at the documented limit: the item reads back as written, got {read}"
    );
    // The same token again is the idempotent repeat, which encodes the same deep item.
    ok(
        &router,
        "TransactWriteItems",
        json!({
            "ClientRequestToken": "nesting-token",
            "TransactItems": [{"Put": {"TableName": table, "Item": item}}],
        }),
    )
    .await;

    let (status, refused) = call(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [{"Put": {
            "TableName": table,
            "Item": {"pk": {"S": "too-deep"}, "tree": nested(limit)},
        }}]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case nesting past the documented limit: expected HTTP 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case nesting past the documented limit: expected a ValidationException, got {refused}"
    );
    let absent = stored(&router, table, &json!({"pk": {"S": "too-deep"}})).await;
    assert_eq!(
        absent,
        Value::Null,
        "case nesting past the documented limit: the refused write stored nothing, got {absent}"
    );
}

/// What the emulator remembers for `ClientRequestToken`s is BUDGETED: the number of
/// tokens a caller can present is caller-chosen, so past this emulator's own ceiling
/// the oldest record is dropped and that token is treated as a new request — exactly as
/// one past the 10-minute window is. The rows are a token inside the budget (replayed,
/// so its `ADD` applies once) and the oldest token once the budget is full (treated as
/// new, so its `ADD` applies again).
#[tokio::test]
async fn the_remembered_transaction_tokens_are_budgeted() {
    let router = emulator();
    let table = "transact-token-budget";
    create_active_table(&router, table, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": table, "Item": {"pk": {"S": "counted"}, "ver": {"N": "1"}}}),
    )
    .await;

    // An ADD is the action a re-application is VISIBLE in, which is why the budget is
    // asserted with one.
    let counted = |token: &str| {
        json!({
            "ClientRequestToken": token,
            "TransactItems": [{"Update": {
                "TableName": table,
                "Key": {"pk": {"S": "counted"}},
                "UpdateExpression": "ADD ver :one",
                "ExpressionAttributeValues": {":one": {"N": "1"}},
            }}],
        })
    };

    ok(&router, "TransactWriteItems", counted("oldest")).await;
    ok(&router, "TransactWriteItems", counted("oldest")).await;
    let read = stored(&router, table, &json!({"pk": {"S": "counted"}})).await;
    assert_eq!(
        read,
        json!({"pk": {"S": "counted"}, "ver": {"N": "2"}}),
        "case a token inside the budget: the retry is answered, not applied, got {read}"
    );

    // Fill the budget with distinct tokens, each writing its own item, so the record of
    // "oldest" is the one evicted.
    let budget = service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_RECORDED_TRANSACTIONS;
    for at in 0..budget {
        ok(
            &router,
            "TransactWriteItems",
            json!({
                "ClientRequestToken": format!("token-{at}"),
                "TransactItems": [{"Put": {
                    "TableName": table,
                    "Item": {"pk": {"S": format!("filler-{at}")}},
                }}],
            }),
        )
        .await;
    }

    ok(&router, "TransactWriteItems", counted("oldest")).await;
    let read = stored(&router, table, &json!({"pk": {"S": "counted"}})).await;
    assert_eq!(
        read,
        json!({"pk": {"S": "counted"}, "ver": {"N": "3"}}),
        "case the oldest token once the budget is full: its record is dropped, so the token \
         is a new request, got {read}"
    );
    // The most recent token is still remembered, so the budget evicts rather than clears.
    ok(
        &router,
        "TransactWriteItems",
        json!({
            "ClientRequestToken": format!("token-{}", budget - 1),
            "TransactItems": [{"Put": {
                "TableName": table,
                "Item": {"pk": {"S": format!("filler-{}", budget - 1)}},
            }}],
        }),
    )
    .await;
    let still_counted = stored(&router, table, &json!({"pk": {"S": "counted"}})).await;
    assert_eq!(
        still_counted,
        json!({"pk": {"S": "counted"}, "ver": {"N": "3"}}),
        "case the newest token: it is still remembered, got {still_counted}"
    );
}

/// A transactional refusal's CLASS says where it was decided: one the transaction
/// reaches while keying the item an element writes CANCELS the transaction, so the
/// answer is `TransactionCanceledException` whose `CancellationReasons` name that
/// element `ValidationError` and every other element `None`; one the service decides
/// about the REQUEST, before any element is applied, is a plain `ValidationException`
/// with no reasons at all.
///
/// "ValidationError - Code: ValidationError, Message: One or more parameter values were
/// invalid" ("Cancellation reason codes and possible error messages", the
/// `TransactWriteItems` doc). The rows span what the refusal is about (an item whose key
/// attribute is not of the type the table declares, a `Key` member that does not name
/// this table's key, an item addressed twice, an entry naming no action), which position
/// in the transaction carries it, and how many elements stand beside it — and in every
/// row the element that would otherwise have landed did not.
#[tokio::test]
async fn a_refusals_class_says_whether_the_transaction_was_cancelled() {
    let router = emulator();
    let table = "transact-failure-class";
    create_active_table(&router, table, &[("pk", "HASH", "S")]).await;

    // The element nothing is wrong with, which must not land in any row.
    let innocent = json!({
        "Put": {"TableName": table, "Item": {"pk": {"S": "innocent"}, "ver": {"N": "1"}}}
    });
    // A `Put` whose ITEM types the partition key as a number, which the table declares
    // as a string: the transaction reaches that when it keys the item.
    let mistyped_item = json!({
        "Put": {"TableName": table, "Item": {"pk": {"N": "1"}, "ver": {"N": "1"}}}
    });

    // One row: what it is called, the elements it sends, the error code it must be
    // answered with, the `CancellationReason$Code`s that answer carries (`None`
    // when a request-level refusal carries no reasons at all), and the `Message` the
    // refused element's reason states.
    struct Row {
        case: &'static str,
        elements: Vec<Value>,
        code: &'static str,
        reasons: Option<Vec<&'static str>>,
        /// What the refused element's reason says. A key attribute of another TYPE than
        /// the schema declares is stated as the mismatch between the two types when it is
        /// read out of an ITEM, and as the one key-schema sentence when it is read out of
        /// a `Key`-shaped member, which names the key as a whole.
        message: &'static str,
    }
    const TYPE_MISMATCH: &str =
        "One or more parameter values were invalid: Type mismatch for key pk expected: S actual: N";
    const KEY_SCHEMA_MISMATCH: &str = "The provided key element does not match the schema";
    let cases: Vec<Row> = vec![
        Row {
            case: "an item typing the key attribute the table does not",
            elements: vec![mistyped_item.clone()],
            code: "TransactionCanceledException",
            reasons: Some(vec!["ValidationError"]),
            message: TYPE_MISMATCH,
        },
        Row {
            case: "that item after an element nothing is wrong with",
            elements: vec![innocent.clone(), mistyped_item.clone()],
            code: "TransactionCanceledException",
            reasons: Some(vec!["None", "ValidationError"]),
            message: TYPE_MISMATCH,
        },
        Row {
            case: "that item before two elements nothing is wrong with",
            elements: vec![
                mistyped_item.clone(),
                innocent.clone(),
                json!({"Delete": {"TableName": table, "Key": {"pk": {"S": "absent"}}}}),
            ],
            code: "TransactionCanceledException",
            reasons: Some(vec!["ValidationError", "None", "None"]),
            message: TYPE_MISMATCH,
        },
        Row {
            case: "a Key member that does not name this table's key",
            elements: vec![
                innocent.clone(),
                json!({"Delete": {"TableName": table, "Key": {"other": {"S": "x"}}}}),
            ],
            code: "ValidationException",
            reasons: None,
            message: KEY_SCHEMA_MISMATCH,
        },
        Row {
            case: "a Key member typing the key attribute the table does not",
            elements: vec![
                innocent.clone(),
                json!({"Update": {
                    "TableName": table,
                    "Key": {"pk": {"N": "1"}},
                    "UpdateExpression": "SET ver = :one",
                    "ExpressionAttributeValues": {":one": {"N": "1"}},
                }}),
            ],
            // A type the TABLE's key schema does not declare is decided against that
            // table, so the element that carried it cancels the transaction.
            code: "TransactionCanceledException",
            reasons: Some(vec!["None", "ValidationError"]),
            message: KEY_SCHEMA_MISMATCH,
        },
        Row {
            case: "one item addressed twice",
            elements: vec![
                innocent.clone(),
                json!({"Delete": {"TableName": table, "Key": {"pk": {"S": "innocent"}}}}),
            ],
            code: "ValidationException",
            reasons: None,
            message: KEY_SCHEMA_MISMATCH,
        },
        Row {
            case: "an entry naming no action",
            elements: vec![innocent.clone(), json!({})],
            code: "ValidationException",
            reasons: None,
            message: KEY_SCHEMA_MISMATCH,
        },
    ];
    for Row {
        case,
        elements,
        code,
        reasons,
        message: expected_message,
    } in cases
    {
        let (status, refused) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": elements}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            code,
            "case {case}: expected {code}, got {refused}"
        );
        match reasons {
            None => assert!(
                refused.get("CancellationReasons").is_none(),
                "case {case}: a request-level refusal carries no reasons, got {refused}"
            ),
            Some(expected) => {
                let reported: Vec<String> = refused["CancellationReasons"]
                    .as_array()
                    .map(|reasons| {
                        reasons
                            .iter()
                            .map(|reason| reason["Code"].as_str().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    reported, expected,
                    "case {case}: expected the reason codes {expected:?}, got {refused}"
                );
                let at = expected
                    .iter()
                    .position(|code| *code == "ValidationError")
                    .expect("a row that cancels names the refused element");
                let message = refused["CancellationReasons"][at]["Message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert_eq!(
                    message, expected_message,
                    "case {case}: expected the refused element's reason to state \
                     {expected_message:?}, got {message:?}"
                );
                for (index, reason) in refused["CancellationReasons"]
                    .as_array()
                    .expect("the reported reasons")
                    .iter()
                    .enumerate()
                {
                    if index != at {
                        assert!(
                            reason.get("Message").is_none(),
                            "case {case}: the element nothing was wrong with carries no \
                             message, got {reason}"
                        );
                    }
                }
            }
        }
        let absent = stored(&router, table, &json!({"pk": {"S": "innocent"}})).await;
        assert_eq!(
            absent,
            Value::Null,
            "case {case}: the element that would have landed did not, got {absent}"
        );
    }
}
