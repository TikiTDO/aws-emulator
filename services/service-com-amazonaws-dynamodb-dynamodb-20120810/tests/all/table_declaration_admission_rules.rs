// SPDX-License-Identifier: CC0-1.0

//! What a TABLE DECLARATION a request carries has to satisfy before the table it declares
//! is stored, over the rules themselves rather than over one recorded request.
//!
//! Four rules, each driven end to end over a table of inputs:
//!
//! 1. a secondary-index list the request CARRIES has to name an index: an empty one
//!    declares a list of no indexes, which is refused with the fixed parameter-value
//!    sentence naming the member — and nothing is stored, so the read that follows reports
//!    the table as missing. A list the request leaves out asks for nothing and is accepted;
//!    2. a capacity figure the service cannot hold is refused as it is read, naming the
//!    member and the value the request supplied, whichever resource stated it. A refused
//!    update leaves the table settled at the capacity it had, and the next legitimate
//!    update applies the pair it asks for;
//! 3. a LOCAL secondary index keys on the table's own partition key and on one sort key of
//!    its own, so a second `HASH` element is out of place in its key schema — while a
//!    GLOBAL secondary index may carry several partition keys ("Each global secondary index
//!    supports up to 4 partition keys and up to 4 sort keys" —
//!    `smithy_query(query="doc", shape="com.amazonaws.dynamodb#CreateTableInput",
//!    member="GlobalSecondaryIndexes")`, against "an index that has the same partition key
//!    as the table, but a different sort key" for the local one);
//! 4. a VECTOR index exists only on an on-demand table, whichever member declared it;
//! 5. every `Date` member a response carries is an epoch instant at MILLISECOND
//!    resolution, whichever shape and whichever operation reports it.
//!
//! Every bound and every message these suites state comes from the model or from AWS's
//! recorded wording, and the bounds are tested AT the published value and one past it.

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_PROVISIONED_CAPACITY_UNITS;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, index_named, message, ok};

/// AWS's fixed sentence for a list member the request carries with no entries, over the
/// member it named.
fn empty_list(member: &str) -> String {
    format!("One or more parameter values were invalid: List of {member} is empty")
}

/// AWS's fixed sentence for a table the endpoint holds none of, over the name the request
/// asked for.
fn not_found(name: &str) -> String {
    format!("Requested resource not found: Table: {name} not found")
}

/// AWS's fixed sentence for a capacity figure the service cannot hold, over the member and
/// the value the request supplied.
fn out_of_bounds(member: &str, value: i64) -> String {
    format!("Given value {value} for {member} is out of bounds")
}

/// One `KeySchemaElement`.
fn key(name: &str, key_type: &str) -> Value {
    json!({"AttributeName": name, "KeyType": key_type})
}

/// The `AttributeDefinitions` declaring every name as a string attribute.
fn definitions(names: &[&str]) -> Vec<Value> {
    names
        .iter()
        .map(|name| json!({"AttributeName": name, "AttributeType": "S"}))
        .collect()
}

/// Assert `body` is a 400 `ValidationException` and answer the message it carries.
fn refusal(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        (status, error_code(body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
    );
    message(body)
}

/// An on-demand `CreateTable` body over a `pk`/`sk` composite key, with `extra`'s members
/// merged into it — the declaration each row of rules 1 and 3 varies.
fn create_request(name: &str, declared: &[&str], extra: &Value) -> Value {
    let mut request = json!({
        "TableName": name,
        "BillingMode": "PAY_PER_REQUEST",
        "KeySchema": [key("pk", "HASH"), key("sk", "RANGE")],
        "AttributeDefinitions": definitions(declared),
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    request
}

/// Assert the endpoint holds no table called `name`: the read that follows a refused create
/// has to report the resource as missing rather than describing a table AWS would never
/// have made.
async fn assert_no_table(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, body) = call(router, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        (status, error_code(&body).as_str(), message(&body).as_str()),
        (400, "ResourceNotFoundException", not_found(name).as_str()),
        "case {case:?}: a refused create stores no table, got {status} with {body}"
    );
    let listed = ok(router, "ListTables", json!({})).await;
    assert!(
        !listed["TableNames"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .contains(&json!(name)),
        "case {case:?}: a refused create leaves the name free, got {listed}"
    );
}

// ===========================================================================
// 1. A list member the request carries with no entries
// ===========================================================================

#[tokio::test]
async fn a_secondary_index_list_the_request_carries_must_name_an_index() {
    let router = emulator();

    // One valid index of each kind, so a row can state one list empty while the other
    // carries an index — the rule is about the member, not about the table having indexes.
    let global = json!({
        "IndexName": "by-label",
        "KeySchema": [key("label", "HASH")],
        "Projection": {"ProjectionType": "ALL"},
    });
    let local = json!({
        "IndexName": "by-stamp",
        "KeySchema": [key("pk", "HASH"), key("stamp", "RANGE")],
        "Projection": {"ProjectionType": "ALL"},
    });
    // Rows differ along WHICH list is empty, whether the other list is absent or carries an
    // index, and how many of them are empty at once. The last column is the member the
    // refusal has to name.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an empty global list alone",
            json!({"GlobalSecondaryIndexes": []}),
            "GlobalSecondaryIndexes",
        ),
        (
            "an empty local list alone",
            json!({"LocalSecondaryIndexes": []}),
            "LocalSecondaryIndexes",
        ),
        (
            "both lists empty",
            json!({"GlobalSecondaryIndexes": [], "LocalSecondaryIndexes": []}),
            "GlobalSecondaryIndexes",
        ),
        (
            "an empty global list beside a local index",
            json!({"GlobalSecondaryIndexes": [], "LocalSecondaryIndexes": [local.clone()]}),
            "GlobalSecondaryIndexes",
        ),
        (
            "an empty local list beside a global index",
            json!({"GlobalSecondaryIndexes": [global.clone()], "LocalSecondaryIndexes": []}),
            "LocalSecondaryIndexes",
        ),
    ];

    for (ordinal, (case, extra, member)) in rows.iter().enumerate() {
        let name = format!("declared-{ordinal}");
        let (status, body) = call(
            &router,
            "CreateTable",
            create_request(&name, &["pk", "sk", "label", "stamp"], extra),
        )
        .await;
        assert_eq!(
            refusal(case, status, &body),
            empty_list(member),
            "case {case:?}: expected the empty-list sentence over {member}, got {body}"
        );
        assert_no_table(&router, case, &name).await;
    }

    // The controls: the same declaration with the member LEFT OUT, and with a list that
    // names one index, is accepted — and the stored table reports exactly the indexes the
    // request named, read back through `DescribeTable`.
    for (case, declared, extra, expected) in [
        (
            "neither list stated",
            &["pk", "sk"][..],
            json!({}),
            Value::Null,
        ),
        (
            "a global list naming one index",
            &["pk", "sk", "label"][..],
            json!({"GlobalSecondaryIndexes": [global.clone()]}),
            json!("by-label"),
        ),
    ] {
        let name = format!("accepted-{}", case.replace(' ', "-"));
        ok(
            &router,
            "CreateTable",
            create_request(&name, declared, &extra),
        )
        .await;
        let described = describe_when_active(&router, case, &name).await;
        assert_eq!(
            index_named(&described, "by-label")["IndexName"],
            expected,
            "case {case:?}: expected the indexes the request named, got {described}"
        );
    }
}

#[tokio::test]
async fn an_import_declaration_carrying_an_empty_index_list_is_refused_too() {
    let router = emulator();

    // The same rule over the OTHER member a table is declared in,
    // `ImportTable$TableCreationParameters`: an empty list there declares a list of no
    // indexes just as it does on the create. The rows differ along the list's state, so the
    // accepted one shows the refusal is about the empty list and not about the import.
    for (case, indexes, refused) in [
        ("an empty global list", Some(json!([])), true),
        ("no list at all", None, false),
    ] {
        let mut parameters = json!({
            "TableName": format!("imported-{}", case.replace(' ', "-")),
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&["pk"]),
            "BillingMode": "PAY_PER_REQUEST",
        });
        if let Some(indexes) = indexes {
            parameters["GlobalSecondaryIndexes"] = indexes;
        }
        let (status, body) = call(
            &router,
            "ImportTable",
            json!({
                "S3BucketSource": {"S3Bucket": "declarations"},
                "InputFormat": "DYNAMODB_JSON",
                "TableCreationParameters": parameters,
            }),
        )
        .await;
        match refused {
            true => assert_eq!(
                refusal(case, status, &body),
                empty_list("GlobalSecondaryIndexes"),
                "case {case:?}: expected the empty-list sentence, got {body}"
            ),
            false => assert_eq!(
                status, 200,
                "case {case:?}: expected the import to start, got {status} with {body}"
            ),
        }
    }
}

// ===========================================================================
// 2. A capacity figure the service cannot hold
// ===========================================================================

/// A provisioned `CreateTable` body at the capacity pair given.
fn provisioned_request(name: &str, read: i64, write: i64) -> Value {
    json!({
        "TableName": name,
        "BillingMode": "PROVISIONED",
        "KeySchema": [key("pk", "HASH")],
        "AttributeDefinitions": definitions(&["pk"]),
        "ProvisionedThroughput": {"ReadCapacityUnits": read, "WriteCapacityUnits": write},
    })
}

#[tokio::test]
async fn a_capacity_figure_past_what_the_service_holds_is_refused_as_it_is_read() {
    let router = emulator();

    // Rows differ along the dimension that is out of bounds, how far past the bound it
    // goes, the operation that carried it and the resource it was stated for (the table's
    // own capacity and a global secondary index's). The published bound itself is tested
    // below, so each row here is one past it or further.
    let past = MAX_PROVISIONED_CAPACITY_UNITS + 1;
    let far_past = MAX_PROVISIONED_CAPACITY_UNITS * 4096;
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a create one past the bound on the read dimension",
            "CreateTable",
            provisioned_request("bounded-create-read", past, 5),
            out_of_bounds("ReadCapacityUnits", past),
        ),
        (
            "a create far past the bound on the write dimension",
            "CreateTable",
            provisioned_request("bounded-create-write", 5, far_past),
            out_of_bounds("WriteCapacityUnits", far_past),
        ),
        (
            "a create whose INDEX asks for more than the service holds",
            "CreateTable",
            json!({
                "TableName": "bounded-create-index",
                "BillingMode": "PROVISIONED",
                "KeySchema": [key("pk", "HASH")],
                "AttributeDefinitions": definitions(&["pk", "label"]),
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
                "GlobalSecondaryIndexes": [{
                    "IndexName": "by-label",
                    "KeySchema": [key("label", "HASH")],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": 5,
                        "WriteCapacityUnits": past,
                    },
                }],
            }),
            out_of_bounds("WriteCapacityUnits", past),
        ),
        (
            "an import declaration past the bound",
            "ImportTable",
            json!({
                "S3BucketSource": {"S3Bucket": "declarations"},
                "InputFormat": "DYNAMODB_JSON",
                "TableCreationParameters": {
                    "TableName": "bounded-import",
                    "BillingMode": "PROVISIONED",
                    "KeySchema": [key("pk", "HASH")],
                    "AttributeDefinitions": definitions(&["pk"]),
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": far_past,
                        "WriteCapacityUnits": 5,
                    },
                },
            }),
            out_of_bounds("ReadCapacityUnits", far_past),
        ),
    ];

    for (case, operation, request, expected) in &rows {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            &refusal(case, status, &body),
            expected,
            "case {case:?}: expected the out-of-bounds sentence, got {body}"
        );
    }

    // AT the published bound the figure is held, so the bound is a ceiling and not an
    // approximation: the table is created and reports the pair it was created with.
    let at_bound = "bounded-at-the-ceiling";
    ok(
        &router,
        "CreateTable",
        provisioned_request(at_bound, MAX_PROVISIONED_CAPACITY_UNITS, 5),
    )
    .await;
    let described = describe_when_active(&router, "at the published bound", at_bound).await;
    assert_eq!(
        described["ProvisionedThroughput"]["ReadCapacityUnits"],
        json!(MAX_PROVISIONED_CAPACITY_UNITS),
        "case at the published bound: expected the capacity it was created with, got {described}"
    );
}

#[tokio::test]
async fn an_update_past_the_bound_leaves_the_table_as_it_was_and_the_next_one_applies() {
    let router = emulator();
    let name = "capacity-moves";
    ok(&router, "CreateTable", provisioned_request(name, 5, 7)).await;
    let settled = describe_when_active(&router, "the table settles before it is updated", name)
        .await;
    // The warm pair in force before any update, read off the table rather than stated here:
    // what the rule says about it is that a REFUSED update leaves it alone, whatever setting
    // it holds.
    let warm_before = settled["WarmThroughput"].clone();

    // Rows differ along the dimension that is out of bounds and how far past the bound it
    // goes; each is refused, and after each the table is still settled at the pair it was
    // created with — a refused update moves nothing, not even the table's status.
    for (case, read, write, member, value) in [
        (
            "an update one past the bound on the read dimension",
            MAX_PROVISIONED_CAPACITY_UNITS + 1,
            7,
            "ReadCapacityUnits",
            MAX_PROVISIONED_CAPACITY_UNITS + 1,
        ),
        (
            "an update far past the bound on the write dimension",
            5,
            MAX_PROVISIONED_CAPACITY_UNITS * 1024,
            "WriteCapacityUnits",
            MAX_PROVISIONED_CAPACITY_UNITS * 1024,
        ),
    ] {
        let (status, body) = call(
            &router,
            "UpdateTable",
            json!({
                "TableName": name,
                "ProvisionedThroughput": {
                    "ReadCapacityUnits": read,
                    "WriteCapacityUnits": write,
                },
            }),
        )
        .await;
        assert_eq!(
            refusal(case, status, &body),
            out_of_bounds(member, value),
            "case {case:?}: expected the out-of-bounds sentence, got {body}"
        );
        let described = ok(&router, "DescribeTable", json!({"TableName": name})).await;
        let table = &described["Table"];
        assert_eq!(
            (
                table["TableStatus"].clone(),
                table["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
                table["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
            ),
            (json!("ACTIVE"), json!(5), json!(7)),
            "case {case:?}: a refused update leaves the table settled at its own capacity, \
             got {described}"
        );
        assert_eq!(
            table["WarmThroughput"], warm_before,
            "case {case:?}: a refused update leaves the warm pair as it was, got {described}"
        );
    }

    // And a legitimate update after them applies the pair it asks for, on BOTH dimensions,
    // rather than retaining either of the figures in force.
    ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": name,
            "ProvisionedThroughput": {"ReadCapacityUnits": 11, "WriteCapacityUnits": 13},
        }),
    )
    .await;
    let settled = describe_when_active(&router, "the update settles", name).await;
    assert_eq!(
        (
            settled["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            settled["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
        ),
        (json!(11), json!(13)),
        "case the update settles: expected the requested capacity on both dimensions, got \
         {settled}"
    );
    assert!(
        settled["ProvisionedThroughput"]["LastIncreaseDateTime"].is_number(),
        "case the update settles: a raised capacity records when it was raised, got {settled}"
    );
}

// ===========================================================================
// 3. The key schema each KIND of secondary index may carry
// ===========================================================================

#[tokio::test]
async fn a_local_index_keys_on_the_tables_partition_key_and_one_sort_key() {
    let router = emulator();

    // Rows differ along the index KIND the schema is declared for and the schema itself:
    // how many partition keys it leads with, whether a sort key follows, and how many.
    // `None` is a schema the kind admits. The local rows all begin with the table's own
    // partition key, so what each one tests is the shape and not the partition key.
    let rows: Vec<(&str, bool, Vec<Value>, Option<String>)> = vec![
        (
            "a local index with the table's partition key and a sort key",
            true,
            vec![key("pk", "HASH"), key("alt", "RANGE")],
            None,
        ),
        (
            "a local index with a second partition key instead of a sort key",
            true,
            vec![key("pk", "HASH"), key("alt", "HASH")],
            Some(
                "Invalid KeySchema for index idx: The second KeySchemaElement is not a RANGE \
                 key type"
                    .to_string(),
            ),
        ),
        (
            "a local index with three partition keys",
            true,
            vec![key("pk", "HASH"), key("alt", "HASH"), key("other", "HASH")],
            Some(
                "Invalid KeySchema for index idx: The second KeySchemaElement is not a RANGE \
                 key type"
                    .to_string(),
            ),
        ),
        (
            "a local index with no sort key at all",
            true,
            vec![key("pk", "HASH")],
            Some(
                "One or more parameter values were invalid: Index KeySchema does not have a \
                 range key for index: idx"
                    .to_string(),
            ),
        ),
        (
            "a local index with two sort keys",
            true,
            vec![key("pk", "HASH"), key("alt", "RANGE"), key("other", "RANGE")],
            Some(
                "Invalid KeySchema for index idx: an index key may declare at most 1 RANGE \
                 attribute(s), got 2"
                    .to_string(),
            ),
        ),
        (
            "a global index with two partition keys, which its kind admits",
            false,
            vec![key("alt", "HASH"), key("other", "HASH")],
            None,
        ),
        (
            "a global index with a partition key alone, which its kind admits",
            false,
            vec![key("alt", "HASH")],
            None,
        ),
    ];

    for (ordinal, (case, local, key_schema, expected)) in rows.iter().enumerate() {
        let name = format!("indexed-{ordinal}");
        let index = json!({
            "IndexName": "idx",
            "KeySchema": key_schema,
            "Projection": {"ProjectionType": "ALL"},
        });
        let member = match local {
            true => "LocalSecondaryIndexes",
            false => "GlobalSecondaryIndexes",
        };
        // Exactly the attributes the table's own key and this index's key use: "Some
        // AttributeDefinitions are not used in any KeySchema" otherwise, which is another
        // rule's refusal and not the one this row is about.
        let mut declared: Vec<&str> = vec!["pk", "sk"];
        for element in key_schema {
            let named = element["AttributeName"].as_str().unwrap_or_default();
            if !declared.contains(&named) {
                declared.push(named);
            }
        }
        let (status, body) = call(
            &router,
            "CreateTable",
            create_request(&name, &declared, &json!({member: [index]})),
        )
        .await;
        match expected {
            Some(expected) => {
                assert_eq!(
                    &refusal(case, status, &body),
                    expected,
                    "case {case:?}: expected the refusal that names the schema's fault, got \
                     {body}"
                );
                assert_no_table(&router, case, &name).await;
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the schema its kind admits to be accepted, got \
                     {status} with {body}"
                );
                // The accepted index is read back with the key schema it was declared with,
                // in the order it was declared: that is what a Query against it keys on.
                let described = describe_when_active(&router, case, &name).await;
                let stored = described[member]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|index| index["IndexName"] == json!("idx"))
                    .unwrap_or(Value::Null);
                assert_eq!(
                    stored["KeySchema"],
                    json!(key_schema),
                    "case {case:?}: expected the declared key schema back, got {described}"
                );
            }
        }
    }
}

// ===========================================================================
// 4. A vector index and the table's capacity mode
// ===========================================================================

#[tokio::test]
async fn a_vector_index_is_only_carried_by_an_on_demand_table() {
    let router = emulator();

    /// AWS's fixed sentence for a vector index on a table that is not on-demand.
    const ONLY_ON_DEMAND: &str = "One or more parameter values were invalid: Vector indexes \
                                  are only supported for PAY_PER_REQUEST tables";
    let index = json!({
        "IndexName": "by-embedding",
        "VectorAttribute": {"AttributeName": "emb"},
        "Dimensions": 4,
        "DistanceFunction": "COSINE",
        "Projection": {"ProjectionType": "ALL"},
    });

    // Rows differ along HOW the declaration states the provisioned capacity mode — the
    // member that names it (`BillingMode`, `TableThroughputMode`) and the default a
    // declaration that names neither takes — since each resolves to the same mode.
    for (case, mode) in [
        ("a table created PROVISIONED", json!({"BillingMode": "PROVISIONED"})),
        (
            "a table created in that mode through TableThroughputMode",
            json!({"TableThroughputMode": "PROVISIONED"}),
        ),
        ("a table created with no mode stated at all", json!({})),
    ] {
        let mut request = json!({
            "TableName": format!("vectors-{}", case.replace(' ', "-")),
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&["pk"]),
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            "VectorIndexes": [index.clone()],
        });
        for (member, value) in mode.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        let (status, body) = call(&router, "CreateTable", request).await;
        assert_eq!(
            refusal(case, status, &body),
            ONLY_ON_DEMAND,
            "case {case:?}: expected the capacity-mode sentence, got {body}"
        );
    }

    // On an ON-DEMAND table the same index is accepted and read back, so the refusal is
    // about the capacity mode and not about the index.
    let name = "vectors-on-demand";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": name,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&["pk"]),
            "VectorIndexes": [index.clone()],
        }),
    )
    .await;
    let described = describe_when_active(&router, "an on-demand table", name).await;
    assert_eq!(
        described["VectorIndexes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .len(),
        1,
        "case an on-demand table: expected the declared vector index back, got {described}"
    );

    // And an UPDATE that would add one to a table in the provisioned mode is refused by the
    // same rule, since the mode is the table's rather than the member's.
    let provisioned = "vectors-updated";
    ok(
        &router,
        "CreateTable",
        provisioned_request(provisioned, 5, 5),
    )
    .await;
    describe_when_active(&router, "the table settles before it is updated", provisioned).await;
    let (status, body) = call(
        &router,
        "UpdateTable",
        json!({
            "TableName": provisioned,
            "VectorIndexUpdates": [{"Create": index}],
        }),
    )
    .await;
    assert_eq!(
        refusal("an update adding one to a provisioned table", status, &body),
        ONLY_ON_DEMAND,
        "case an update adding one to a provisioned table: expected the capacity-mode \
         sentence, got {body}"
    );
}

// ===========================================================================
// 5. The resolution a `Date` member of a response is stated at
// ===========================================================================

/// Whether `value` is an epoch instant at MILLISECOND resolution: a number whose decimal
/// spelling carries at most three fractional digits, which is the finest a `Date` member of
/// a DynamoDB response states ("in UNIX epoch time format" —
/// `smithy_query(query="doc", shape="com.amazonaws.dynamodb#TableDescription",
/// member="CreationDateTime")`).
fn at_millisecond_resolution(value: &Value) -> bool {
    let Some(seconds) = value.as_f64() else {
        return false;
    };
    format!("{seconds}")
        .split_once('.')
        .is_none_or(|(_, fraction)| fraction.len() <= 3)
}

#[tokio::test]
async fn every_date_member_a_response_carries_is_stated_at_millisecond_resolution() {
    let router = emulator();

    // Rows differ along the member, the shape it sits in and the operation that reports it:
    // the instant a table was created, the instant its capacity mode was switched, the
    // instant its capacity was last raised, and the instant a backup of it was taken — each
    // read off the response that carries it, so the rule is asserted over the whole family
    // rather than over one member.
    let created = ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "stamped",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [key("pk", "HASH")],
            "AttributeDefinitions": definitions(&["pk"]),
        }),
    )
    .await;
    let described = describe_when_active(&router, "a created table", "stamped").await;
    let backed_up = ok(
        &router,
        "CreateBackup",
        json!({"TableName": "stamped", "BackupName": "nightly"}),
    )
    .await;

    let provisioned = "stamped-provisioned";
    ok(&router, "CreateTable", provisioned_request(provisioned, 5, 7)).await;
    describe_when_active(&router, "a provisioned table", provisioned).await;
    let raised = ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": provisioned,
            "ProvisionedThroughput": {"ReadCapacityUnits": 9, "WriteCapacityUnits": 11},
        }),
    )
    .await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "the create's own creation instant",
            created["TableDescription"]["CreationDateTime"].clone(),
        ),
        (
            "the create's capacity-mode switch",
            created["TableDescription"]["BillingModeSummary"]
                ["LastUpdateToPayPerRequestDateTime"]
                .clone(),
        ),
        (
            "the description's creation instant",
            described["CreationDateTime"].clone(),
        ),
        (
            "the description's capacity-mode switch",
            described["BillingModeSummary"]["LastUpdateToPayPerRequestDateTime"].clone(),
        ),
        (
            "the backup's creation instant",
            backed_up["BackupDetails"]["BackupCreationDateTime"].clone(),
        ),
        (
            "the update's last capacity increase",
            raised["TableDescription"]["ProvisionedThroughput"]["LastIncreaseDateTime"].clone(),
        ),
    ];

    for (case, value) in &rows {
        assert!(
            at_millisecond_resolution(value),
            "case {case:?}: expected an epoch instant at millisecond resolution, got {value}"
        );
    }
}
