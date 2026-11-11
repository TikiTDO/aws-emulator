// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a key defect earns, WHICH answer states it, and the bound a batched
//! write's `RequestItems` is held to.
//!
//! Every suite here is a TABLE spanning what its rule generalises over rather than one
//! recorded request, and every assertion names its case and the value it actually read.
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front doors use.
//!
//! RULE 1 — an `Item` a SINGLE-item write carries is the whole item, so a key attribute it
//! gives no value for is reported as the key it is missing, by name: "One or more
//! parameter values were invalid: Missing the key <name> in the item", the same template
//! the type mismatch on that path states. The rows span the key's role in the schema, the
//! schema's shape, and an item that gives no value for either key.
//!
//! RULE 2 — WHICH side decided a bad key decides which answer states it inside a
//! transaction: a key the TABLE's key schema does not admit (a type it does not declare
//! for that attribute, a value past the size limit of the role it gives it) cancels the
//! element with the documented `ValidationError` reason, while a member that is not shaped
//! like a key at all (a key attribute no value was given for, a value that is no value of
//! its own type) is the request's own defect and refuses the call. The rows span the
//! defect class, the action the key arrived in, and both transactional surfaces.
//!
//! RULE 3 — a refusal about a VALUE a request carried is stated inside the
//! `<N> validation error(s) detected: ` envelope by the operations that WRITE an item and
//! as the sentence alone by the operations that READ one; a value in the `Key` of ONE
//! ELEMENT of a multi-item request is that element's member rather than the request's, so
//! it is stated alone there too. The rows span the operation, the member the value arrived
//! in and the malformed numeric texts.
//!
//! RULE 4 — "Edits an existing item's attributes, or adds a new item to the table if it
//! does not already exist" is read as what the update WRITES: an update that states
//! clauses and gives no value to the key it addresses adds no item, while one that sets or
//! adds an attribute adds it, and one that states no clause at all adds the key's item.
//! The rows span the clause (`REMOVE`, `DELETE`, `SET`, `ADD`, none), the spelling
//! (`UpdateExpression` and the legacy `AttributeUpdates`) and `ReturnValues`.
//!
//! RULE 5 — `BatchWriteItem` holds the WRITE REQUESTS its `RequestItems` carries to the
//! 1..25 bound the model declares for that member, counted across every table the map
//! groups them under, and reports a violation of it over the member path the request
//! validator builds. The rows span both ends of the bound, the bound itself, and how the
//! requests are spread over tables.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The clause every refusal of a parameter value in this service opens with.
const INVALID_PARAMETER: &str = "One or more parameter values were invalid";

/// The one sentence a key that is not the table's primary key is refused with.
const KEY_SCHEMA_MISMATCH: &str = "The provided key element does not match the schema";

/// Call `operation`, require a 400 `ValidationException`, and answer its message.
async fn refusal(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> String {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
         {answered}"
    );
    message(&answered)
}

// ===========================================================================
// RULE 1. A single-item write names the key its item gave no value for
// ===========================================================================

#[tokio::test]
async fn a_single_item_write_names_the_key_the_item_gave_no_value_for() {
    let router = emulator();
    create_active_table(&router, "Simple", &[("pk", "HASH", "S")]).await;
    create_active_table(
        &router,
        "Composite",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
    )
    .await;

    // Each row: the case, the table, the item the write carries, and the key the refusal
    // must name. The rows span the key's role (partition and sort), the schema's shape
    // (simple and composite) and an item that gives no value for either key — where the
    // key named is the first the schema declares.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an item with no key attribute at all",
            "Simple",
            json!({"note": {"S": "plain"}}),
            "pk",
        ),
        ("an empty item", "Simple", json!({}), "pk"),
        (
            "an item omitting the sort key",
            "Composite",
            json!({"pk": {"S": "k"}, "note": {"S": "plain"}}),
            "sk",
        ),
        (
            "an item omitting the partition key",
            "Composite",
            json!({"sk": {"N": "1"}}),
            "pk",
        ),
        (
            "an item omitting both keys",
            "Composite",
            json!({"note": {"S": "plain"}}),
            "pk",
        ),
    ];

    for (case, table, item, named) in rows {
        let stated = refusal(
            &router,
            case,
            "PutItem",
            json!({"TableName": table, "Item": item}),
        )
        .await;
        let expected = format!("{INVALID_PARAMETER}: Missing the key {named} in the item");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // The same item through the BATCHED write is held to the schema as a whole, so it
    // states the one key-schema sentence instead: the two members are refused in the
    // terms each is stated in.
    let stated = refusal(
        &router,
        "an item omitting the sort key, batched",
        "BatchWriteItem",
        json!({"RequestItems": {"Composite": [{"PutRequest": {"Item": {"pk": {"S": "k"}}}}]}}),
    )
    .await;
    assert_eq!(
        stated, KEY_SCHEMA_MISMATCH,
        "case an item omitting the sort key, batched: expected {KEY_SCHEMA_MISMATCH:?}, got \
         {stated:?}"
    );

    // An item that gives every key a value of the declared type is written, so the rows
    // above are about the keys they omitted and not about the operation.
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Composite", "Item": {"pk": {"S": "k"}, "sk": {"N": "1"}}}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "Composite", "Key": {"pk": {"S": "k"}, "sk": {"N": "1"}}}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"pk": {"S": "k"}, "sk": {"N": "1"}}),
        "case a complete item: expected the item back, got {read}"
    );
}

// ===========================================================================
// RULE 2. The side that decided a bad key decides the answer
// ===========================================================================

/// Whether a transaction answers a bad key by CANCELLING the element that carried it or
/// by refusing the whole request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Cancels,
    Refuses,
}

#[tokio::test]
async fn a_transactional_key_is_answered_by_the_side_that_decided_it() {
    let router = emulator();
    create_active_table(
        &router,
        "Keys",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "S")],
    )
    .await;
    // "The maximum length of any partition key value is 2048 bytes" ("Service, Account,
    // and Table Quotas" in the Amazon DynamoDB Developer Guide), so this is one past it.
    let oversize = "x".repeat(2049);

    // Each row: the case, the whole transaction, which answer it earns, and the sentence
    // that answer states. The rows span the defect class (a type the schema does not
    // declare, a value past the role's size limit, a key no value was given for, a number
    // whose text is no number), the action the key arrived in (`Put`'s item, `Delete`,
    // `ConditionCheck`, `Update`, `Get`) and both transactional surfaces.
    let rows: Vec<(&str, &str, Value, Answer, String)> = vec![
        (
            "a put item whose partition key has a type the schema does not declare",
            "TransactWriteItems",
            json!({"TransactItems": [{"Put": {"TableName": "Keys",
                                              "Item": {"pk": {"N": "1"}, "sk": {"S": "s"}}}}]}),
            Answer::Cancels,
            // Read out of an ITEM, a key attribute of another type is stated as the
            // mismatch between the declared and the supplied type; the `Key`-shaped
            // members below name the key as a whole and state the one key-schema sentence.
            format!("{INVALID_PARAMETER}: Type mismatch for key pk expected: S actual: N"),
        ),
        (
            "a delete key whose sort key has a type the schema does not declare",
            "TransactWriteItems",
            json!({"TransactItems": [{"Delete": {"TableName": "Keys",
                                                 "Key": {"pk": {"S": "k"}, "sk": {"N": "1"}}}}]}),
            Answer::Cancels,
            KEY_SCHEMA_MISMATCH.to_string(),
        ),
        (
            "a condition check key of a type the schema does not declare",
            "TransactWriteItems",
            json!({"TransactItems": [{"ConditionCheck": {
                "TableName": "Keys",
                "Key": {"pk": {"S": "k"}, "sk": {"N": "1"}},
                "ConditionExpression": "attribute_exists(pk)"}}]}),
            Answer::Cancels,
            KEY_SCHEMA_MISMATCH.to_string(),
        ),
        (
            "a get key of a type the schema does not declare",
            "TransactGetItems",
            json!({"TransactItems": [{"Get": {"TableName": "Keys",
                                              "Key": {"pk": {"S": "k"}, "sk": {"N": "1"}}}}]}),
            Answer::Cancels,
            KEY_SCHEMA_MISMATCH.to_string(),
        ),
        (
            "a delete key past the partition key's documented size limit",
            "TransactWriteItems",
            json!({"TransactItems": [{"Delete": {
                "TableName": "Keys",
                "Key": {"pk": {"S": oversize.clone()}, "sk": {"S": "s"}}}}]}),
            Answer::Cancels,
            format!(
                "{INVALID_PARAMETER}: Size of hashkey has exceeded the maximum size limit \
                 of2048 bytes"
            ),
        ),
        (
            "a put item that gives no value for either key",
            "TransactWriteItems",
            json!({"TransactItems": [{"Put": {"TableName": "Keys", "Item": {}}}]}),
            Answer::Refuses,
            KEY_SCHEMA_MISMATCH.to_string(),
        ),
        (
            "a delete key that gives no value for the sort key",
            "TransactWriteItems",
            json!({"TransactItems": [{"Delete": {"TableName": "Keys",
                                                 "Key": {"pk": {"S": "k"}}}}]}),
            Answer::Refuses,
            KEY_SCHEMA_MISMATCH.to_string(),
        ),
        (
            "a get key that gives no value for the sort key",
            "TransactGetItems",
            json!({"TransactItems": [{"Get": {"TableName": "Keys",
                                              "Key": {"pk": {"S": "k"}}}}]}),
            Answer::Refuses,
            KEY_SCHEMA_MISMATCH.to_string(),
        ),
        (
            "a delete key naming an attribute the schema does not declare",
            "TransactWriteItems",
            json!({"TransactItems": [{"Delete": {
                "TableName": "Keys",
                "Key": {"pk": {"S": "k"}, "sk": {"S": "s"}, "spare": {"S": "x"}}}}]}),
            Answer::Refuses,
            KEY_SCHEMA_MISMATCH.to_string(),
        ),
    ];

    for (case, operation, request, answer, stated) in rows {
        let (status, answered) = call(&router, operation, request).await;
        if answer == Answer::Refuses {
            assert_eq!(
                (status, error_code(&answered).as_str(), message(&answered)),
                (400, "ValidationException", stated.clone()),
                "case {case:?} ({operation}): expected the request refused with {stated:?}, got \
                 {status} with {answered}"
            );
            assert!(
                answered.get("CancellationReasons").is_none(),
                "case {case:?} ({operation}): a request-level refusal cancels nothing, got \
                 {answered}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "TransactionCanceledException"),
            "case {case:?} ({operation}): expected the transaction cancelled, got {status} with \
             {answered}"
        );
        assert_eq!(
            answered["CancellationReasons"],
            json!([{"Code": "ValidationError", "Message": stated}]),
            "case {case:?} ({operation}): expected the element's reason to state {stated:?}, got \
             {answered}"
        );
    }
}

// ===========================================================================
// RULE 3. A read states a value refusal bare, a write inside the envelope
// ===========================================================================

#[tokio::test]
async fn a_value_refusal_carries_the_envelope_on_a_write_and_not_on_a_read() {
    let router = emulator();
    create_active_table(&router, "Numbers", &[("pk", "HASH", "N")]).await;

    // Each row: the `N` text a key carries, and the sentence its rule earns. The rows
    // span the numeric rules a text can break (it is no number at all, in two spellings,
    // and it carries more significant digits than the documented 38).
    let texts: Vec<(&str, String)> = vec![
        (
            "abc",
            "The parameter cannot be converted to a numeric value: abc".to_string(),
        ),
        (
            "1.2.3",
            "The parameter cannot be converted to a numeric value: 1.2.3".to_string(),
        ),
        (
            "123456789012345678901234567890123456789",
            "Attempting to store more than 38 significant digits in a Number".to_string(),
        ),
    ];

    for (text, clause) in texts {
        // The READS: the sentence alone.
        for (case, operation, request) in [
            (
                "GetItem",
                "GetItem",
                json!({"TableName": "Numbers", "Key": {"pk": {"N": text}}}),
            ),
            (
                "a batched get",
                "BatchGetItem",
                json!({"RequestItems": {"Numbers": {"Keys": [{"pk": {"N": text}}]}}}),
            ),
            (
                "a transactional get",
                "TransactGetItems",
                json!({"TransactItems": [{"Get": {"TableName": "Numbers",
                                                  "Key": {"pk": {"N": text}}}}]}),
            ),
        ] {
            let case = format!("{text:?} read through {case}");
            let stated = refusal(&router, &case, operation, request).await;
            assert_eq!(
                stated, clause,
                "case {case:?}: expected the sentence alone, got {stated:?}"
            );
        }

        // The WRITES: the same sentence inside the envelope that counts the clauses.
        let enveloped = format!("1 validation error detected: {clause}");
        for (case, operation, request) in [
            (
                "DeleteItem",
                "DeleteItem",
                json!({"TableName": "Numbers", "Key": {"pk": {"N": text}}}),
            ),
            (
                "PutItem",
                "PutItem",
                json!({"TableName": "Numbers", "Item": {"pk": {"N": text}}}),
            ),
            (
                "UpdateItem",
                "UpdateItem",
                json!({"TableName": "Numbers", "Key": {"pk": {"N": text}},
                       "UpdateExpression": "SET note = :v",
                       "ExpressionAttributeValues": {":v": {"S": "x"}}}),
            ),
            (
                // The ITEM a transactional `Put` writes is held to the rules a written
                // item is held to everywhere, so its clause keeps the envelope too.
                "the item of a transactional put",
                "TransactWriteItems",
                json!({"TransactItems": [{"Put": {"TableName": "Numbers",
                                                  "Item": {"pk": {"S": "k"},
                                                           "v": {"N": text}}}}]}),
            ),
        ] {
            let case = format!("{text:?} written through {case}");
            let stated = refusal(&router, &case, operation, request).await;
            assert_eq!(
                stated, enveloped,
                "case {case:?}: expected the enveloped clause, got {stated:?}"
            );
        }

        // An ELEMENT's own `Key` is that element's member rather than the request's, so
        // its clause is stated outside the envelope on the transactional write as well.
        for (case, request) in [
            (
                "a transactional delete's key",
                json!({"TransactItems": [{"Delete": {"TableName": "Numbers",
                                                     "Key": {"pk": {"N": text}}}}]}),
            ),
            (
                "a transactional condition check's key",
                json!({"TransactItems": [{"ConditionCheck": {
                    "TableName": "Numbers", "Key": {"pk": {"N": text}},
                    "ConditionExpression": "attribute_exists(pk)"}}]}),
            ),
        ] {
            let case = format!("{text:?} in {case}");
            let stated = refusal(&router, &case, "TransactWriteItems", request).await;
            assert_eq!(
                stated, clause,
                "case {case:?}: expected the sentence alone, got {stated:?}"
            );
        }
    }

    // A key whose text IS a number is served either way, so the rows above are about the
    // texts and not about the operations.
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Numbers", "Item": {"pk": {"N": "1e2"}}}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "Numbers", "Key": {"pk": {"N": "100"}}}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"pk": {"N": "100"}}),
        "case a numeric key: the two spellings address one item, got {read}"
    );
}

// ===========================================================================
// RULE 4. What an update WRITES decides whether it adds an item
// ===========================================================================

#[tokio::test]
async fn an_update_adds_an_item_only_where_a_clause_gave_it_a_value() {
    let router = emulator();
    create_active_table(
        &router,
        "Upserted",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
    )
    .await;

    // Each row: the case, the members the update carries beside its key, and the item the
    // key holds afterwards (`None` for a key that still holds none). The rows span the
    // clause an update can state, both spellings of an update, and the updates that add
    // nothing beside the ones that add the item.
    let rows: Vec<(&str, Value, Option<Value>)> = vec![
        (
            "a sole REMOVE of an attribute that is not there",
            json!({"UpdateExpression": "REMOVE note"}),
            None,
        ),
        (
            "two REMOVEs of attributes that are not there",
            json!({"UpdateExpression": "REMOVE note, memo"}),
            None,
        ),
        (
            "a legacy DELETE of an attribute that is not there",
            json!({"AttributeUpdates": {"note": {"Action": "DELETE"}}}),
            None,
        ),
        (
            "a DELETE of members of a set that is not there",
            json!({"UpdateExpression": "DELETE tags :t",
                   "ExpressionAttributeValues": {":t": {"SS": ["a"]}}}),
            None,
        ),
        (
            "a SET beside a REMOVE",
            json!({"UpdateExpression": "SET note = :v REMOVE memo",
                   "ExpressionAttributeValues": {":v": {"S": "x"}}}),
            Some(json!({"note": {"S": "x"}})),
        ),
        (
            "an ADD of a number",
            json!({"UpdateExpression": "ADD tally :one",
                   "ExpressionAttributeValues": {":one": {"N": "3"}}}),
            Some(json!({"tally": {"N": "3"}})),
        ),
        (
            "a legacy PUT",
            json!({"AttributeUpdates": {"note": {"Action": "PUT", "Value": {"S": "x"}}}}),
            Some(json!({"note": {"S": "x"}})),
        ),
        (
            "an update that states no clause at all",
            json!({}),
            Some(json!({})),
        ),
    ];

    for (index, (case, members, expected)) in rows.into_iter().enumerate() {
        let key = json!({"pk": {"S": format!("row-{index}")}, "sk": {"N": "1"}});
        let mut request = json!({"TableName": "Upserted", "Key": key.clone(),
                                 "ReturnValues": "ALL_NEW"});
        for (member, value) in members.as_object().expect("the update's members are a map") {
            request[member.clone()] = value.clone();
        }
        let answered = ok(&router, "UpdateItem", request).await;

        // What the key holds afterwards is the key attributes plus whatever a clause gave
        // it a value for, and `ALL_NEW` reports exactly that item — or nothing at all when
        // the update added none.
        let stored = ok(
            &router,
            "GetItem",
            json!({"TableName": "Upserted", "Key": key.clone()}),
        )
        .await;
        match expected {
            None => {
                assert_eq!(
                    answered.get("Attributes"),
                    None,
                    "case {case:?}: an update that adds no item reports no attributes, got \
                     {answered}"
                );
                assert_eq!(
                    stored.get("Item"),
                    None,
                    "case {case:?}: the key must still hold no item, got {stored}"
                );
            }
            Some(written) => {
                let mut whole = key.clone();
                for (attribute, value) in
                    written.as_object().expect("the written attributes are a map")
                {
                    whole[attribute.clone()] = value.clone();
                }
                assert_eq!(
                    answered["Attributes"], whole,
                    "case {case:?}: ALL_NEW reports the item the update left, got {answered}"
                );
                assert_eq!(
                    stored["Item"], whole,
                    "case {case:?}: the key holds the item the update added, got {stored}"
                );
            }
        }
    }

    // The same REMOVE on a key that DOES hold an item removes the attribute and leaves
    // the item, so the rows above are about the item that was not there.
    let key = json!({"pk": {"S": "present"}, "sk": {"N": "1"}});
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Upserted",
               "Item": {"pk": {"S": "present"}, "sk": {"N": "1"}, "note": {"S": "x"}}}),
    )
    .await;
    let answered = ok(
        &router,
        "UpdateItem",
        json!({"TableName": "Upserted", "Key": key.clone(),
               "UpdateExpression": "REMOVE note", "ReturnValues": "ALL_NEW"}),
    )
    .await;
    assert_eq!(
        answered["Attributes"], key,
        "case a REMOVE on a stored item: the item keeps its key and loses the attribute, got \
         {answered}"
    );
}

// ===========================================================================
// RULE 5. The write requests a batch carries are held to the declared bound
// ===========================================================================

/// `count` write requests for `table`, each putting a distinct item.
fn write_requests(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"PutRequest": {"Item": {"pk": {"S": format!("k{index}")}}}}))
            .collect(),
    )
}

#[tokio::test]
async fn a_batched_write_holds_its_request_map_to_the_declared_bound() {
    let router = emulator();
    create_active_table(&router, "Bounded", &[("pk", "HASH", "S")]).await;
    create_active_table(&router, "Bounded2", &[("pk", "HASH", "S")]).await;

    // The member path the request validator builds for that map, which its clauses name.
    let member = "requestItems";
    // The bound the model declares for it (`smithy_query(query="operation",
    // operation="BatchWriteItem")` gives RequestItems @length min 1 max 25).
    let (least, most) = (1usize, 25usize);

    // Each row: the case, the map the request sends, and the requirement the clause must
    // state — `None` for a map inside the bound, which is served. The rows span both ends
    // of the bound, the bound itself, and how the requests are spread over tables.
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "a map naming no table at all",
            json!({}),
            Some(format!("Member must have length greater than or equal to {least}")),
        ),
        (
            "a map whose one table was given no request",
            json!({"Bounded": []}),
            Some(format!("Member must have length greater than or equal to {least}")),
        ),
        (
            "a map whose two tables were both given no request",
            json!({"Bounded": [], "Bounded2": []}),
            Some(format!("Member must have length greater than or equal to {least}")),
        ),
        (
            "one request for one table, at the bound's minimum",
            json!({"Bounded": write_requests(1)}),
            None,
        ),
        (
            "requests filling the bound for one table",
            json!({"Bounded": write_requests(most)}),
            None,
        ),
        (
            "one request past the bound for one table",
            json!({"Bounded": write_requests(most + 1)}),
            Some(format!("Member must have length less than or equal to {most}")),
        ),
        (
            "half again past the bound for one table",
            json!({"Bounded": write_requests(most + most / 2)}),
            Some(format!("Member must have length less than or equal to {most}")),
        ),
        (
            "the bound spread over two tables, one request past it",
            json!({"Bounded": write_requests(most), "Bounded2": write_requests(1)}),
            Some(format!("Member must have length less than or equal to {most}")),
        ),
    ];

    for (case, request_items, requirement) in rows {
        let (status, answered) = call(
            &router,
            "BatchWriteItem",
            json!({"RequestItems": request_items.clone()}),
        )
        .await;
        let Some(requirement) = requirement else {
            assert_eq!(
                (status, answered["UnprocessedItems"].clone()),
                (200, json!({})),
                "case {case:?}: a map inside the bound is written in full, got {status} with \
                 {answered}"
            );
            continue;
        };
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
        );
        let stated = message(&answered);
        assert!(
            stated.starts_with("1 validation error detected: ")
                && stated.contains(&format!("at '{member}' failed to satisfy constraint: "))
                && stated.ends_with(&requirement),
            "case {case:?}: expected one clause naming {member:?} and {requirement:?}, got \
             {stated:?}"
        );
    }
}

/// The attribute-value frame the refusals of that member render a value in: every field of
/// the service's own attribute value object, in its order, `field` carrying `value` and
/// every other unset.
///
/// The field list is the one the recorded refusals of the batched and transactional
/// members state (`issues/things-that-now-work/20261023T142648Z-dynamodb-transactgetitems-\
/// and-transactwriteitems-cap-validation-messages-diverge-from.md` quotes it in full); six
/// of the fields belong to no member this service's model declares
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")` gives
/// `S N B SS NS BS M L NULL BOOL`), so nothing a request sends can fill one.
fn frame(field: &str, value: &str) -> String {
    const FIELDS: [&str; 16] = [
        "s", "n", "b", "iNT", "dECIMAL", "dOUBLE", "sS", "nS", "bS", "iNTSET", "dECIMALSET",
        "dOUBLESET", "m", "l", "nULL", "bOOL",
    ];
    let rendered: Vec<String> = FIELDS
        .iter()
        .map(|declared| {
            if *declared == field {
                format!("{declared}={value}")
            } else {
                format!("{declared}=null")
            }
        })
        .collect();
    format!("AttributeValue({})", rendered.join(", "))
}

/// An attribute value nested `depth` maps deep, as the JSON a request sends.
fn nested_value(depth: usize) -> Value {
    let mut value = json!({"S": "bottom"});
    for _ in 0..depth {
        value = json!({"M": {"inner": value}});
    }
    value
}

#[tokio::test]
async fn a_refused_request_map_renders_the_values_the_request_nested() {
    let router = emulator();
    create_active_table(&router, "Nested", &[("pk", "HASH", "S")]).await;

    // One request past the bound, carrying a value the request nested a map inside a list
    // inside a map: the clause states the whole structure, each level in the same frame.
    let mut requests = write_requests(25).as_array().cloned().unwrap_or_default();
    requests.push(json!({"PutRequest": {"Item": {
        "pk": {"S": "nested"},
        "doc": {"M": {"inner": {"L": [{"S": "a"}, {"NULL": true}]}}},
    }}}));
    let (status, answered) = call(
        &router,
        "BatchWriteItem",
        json!({"RequestItems": {"Nested": requests}}),
    )
    .await;
    let stated = message(&answered);
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case a nested value past the bound: expected 400 ValidationException, got {status} \
         with {answered}"
    );
    let elements = format!("[{}, {}]", frame("s", "a"), frame("nULL", "true"));
    let expected = format!(
        "doc={}",
        frame("m", &format!("{{inner={}}}", frame("l", &elements)))
    );
    assert!(
        stated.contains(&expected),
        "case a nested value past the bound: expected the clause to state {expected:?}, got \
         {stated:?}"
    );

    // The nesting is the CALLER's, so the walk that renders it is bounded by nothing the
    // request chose: a value nested far past the documented 32 levels is still answered
    // with a refusal rather than taking the emulator down with it.
    let mut requests = write_requests(25).as_array().cloned().unwrap_or_default();
    requests.push(json!({"PutRequest": {"Item": {
        "pk": {"S": "deep"},
        "doc": nested_value(100),
    }}}));
    let (status, answered) = call(
        &router,
        "BatchWriteItem",
        json!({"RequestItems": {"Nested": requests}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case a value nested past every documented level: expected 400 ValidationException, \
         got {status} with {answered}"
    );

    // A map inside the bound is still served, so the rows above are about the bound and
    // not about the nesting.
    let served = ok(
        &router,
        "BatchWriteItem",
        json!({"RequestItems": {"Nested": [{"PutRequest": {"Item": {
            "pk": {"S": "served"},
            "doc": {"M": {"inner": {"L": [{"S": "a"}]}}},
        }}}]}}),
    )
    .await;
    assert_eq!(
        served["UnprocessedItems"],
        json!({}),
        "case a nested value inside the bound: expected it written in full, got {served}"
    );
}
