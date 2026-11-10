// SPDX-License-Identifier: CC0-1.0

//! Requests the service REFUSES: the rules a request's own members have to satisfy
//! before any state is read or written.
//!
//! The rule these suites span: a request whose members contradict each other, or carry a
//! value outside the domain the model and the docs give that member, is refused with a
//! 400 `ValidationException` and nothing of it is applied — whichever operation carried
//! it, and whether or not the addressed table holds anything to act on. The recorded
//! report cited one request per rule; each suite below drives the rule over the set of
//! inputs it generalises over:
//!
//! * `a_projection_of_include_names_the_attributes_it_projects` — the member pair both
//!   ways, over every projection type, every kind of secondary index, and both the
//!   operation that creates an index and the one that adds one.
//! * `a_table_a_refused_create_did_not_make_is_not_found` — the refusal leaves no state,
//!   so the reader reports the table as missing.
//! * `a_type_test_names_a_type_code` — every code the `AttributeValue` union declares,
//!   and tokens that are no code, on the expression surface and the statement surface.
//! * `an_index_a_request_adds_declares_its_key_attributes` — the key attributes of a new
//!   index, declared and left out.
//! * `a_statement_clause_states_one_action` — the comma that is one token past a
//!   complete statement, beside the spellings that are not.
//! * `a_substitution_table_defines_something` — an empty table and an empty name, over
//!   every operation that takes them.
//! * `a_time_range_runs_forwards` — the two bounds in every order, and either alone.
//! * `every_value_a_request_carries_obeys_the_rules_for_its_type` — the numeric domain
//!   and the set rules, over the item, the expression values and a statement's
//!   parameters.
//! * `an_arn_addresses_this_endpoints_own_region` — the ARN spelling of a table name,
//!   over several operations and several regions.
//! * `a_configuration_change_changes_something` — the precision already in force.
//! * `an_expression_names_attributes_rather_than_literals` — a numeric literal where a
//!   path belongs.
//! * `a_request_states_its_intent_one_way` — the legacy members beside the expression
//!   members.
//! * `an_expected_entry_states_what_it_expects` — the `Expected` member's combinations.

use serde_json::{json, Value};

use crate::emulator::{call, call_in, emulator, error_code, ok, REGION};
use crate::probe::{probe_table as table, refusal, refusal_coded};
use crate::table_items::create_active_table;

/// 39 significant digits: one past the documented precision of an `N` value.
const THIRTY_NINE_DIGITS: &str = "123456789012345678901234567890123456789";

// ===========================================================================
// 1. A projection of INCLUDE names the attributes it projects
// ===========================================================================

/// The projection types the model declares for `Projection$ProjectionType`, read with
/// `smithy_query(query="shape",shape="Projection")`: `KEYS_ONLY`, `INCLUDE`, `ALL`.
const PROJECTION_TYPES: [&str; 3] = ["KEYS_ONLY", "INCLUDE", "ALL"];

/// AWS's fixed text for an `INCLUDE` projection that names no `NonKeyAttributes`.
const INCLUDE_WITHOUT_ATTRIBUTES: &str = "One or more parameter values were invalid: \
                                          ProjectionType is INCLUDE, but NonKeyAttributes is \
                                          not specified";

/// A create carrying one global secondary index with `projection`.
fn create_with_index_projection(name: &str, projection: Value) -> Value {
    json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "gk", "AttributeType": "S"},
        ],
        "BillingMode": "PAY_PER_REQUEST",
        "GlobalSecondaryIndexes": [{
            "IndexName": "by-gk",
            "KeySchema": [{"AttributeName": "gk", "KeyType": "HASH"}],
            "Projection": projection,
        }],
    })
}

#[tokio::test]
async fn a_projection_of_include_names_the_attributes_it_projects() {
    let router = emulator();

    // Every projection type the model declares, with and without the attribute list:
    // the pair holds together only when INCLUDE and a non-empty list come together.
    for projection_type in PROJECTION_TYPES {
        let bare = create_with_index_projection(
            &format!("proj-bare-{projection_type}"),
            json!({"ProjectionType": projection_type}),
        );
        let case = format!("{projection_type} naming no NonKeyAttributes");
        if projection_type == "INCLUDE" {
            let stated = refusal(&router, &case, "CreateTable", bare).await;
            assert_eq!(
                stated, INCLUDE_WITHOUT_ATTRIBUTES,
                "case {case:?}: expected the parameter-value sentence, got {stated:?}"
            );
        } else {
            ok(&router, "CreateTable", bare).await;
        }

        let listed = create_with_index_projection(
            &format!("proj-listed-{projection_type}"),
            json!({"ProjectionType": projection_type, "NonKeyAttributes": ["note"]}),
        );
        let case = format!("{projection_type} naming NonKeyAttributes");
        if projection_type == "INCLUDE" {
            ok(&router, "CreateTable", listed).await;
        } else {
            // The other half of the same rule, which the service already stated.
            let stated = refusal(&router, &case, "CreateTable", listed).await;
            assert!(
                stated.contains("NonKeyAttributes can only be specified"),
                "case {case:?}: expected the member pair to be refused, got {stated:?}"
            );
        }
    }

    // A projection with no type at all defaults to ALL ("When using the DynamoDB
    // console, ALL is selected by default"), so it needs no attribute list.
    ok(
        &router,
        "CreateTable",
        create_with_index_projection("proj-default", json!({})),
    )
    .await;

    // The same rule for a LOCAL secondary index, which carries the same `Projection`.
    let case = "a local secondary index projecting INCLUDE with no attributes";
    let stated = refusal(
        &router,
        case,
        "CreateTable",
        json!({
            "TableName": "proj-lsi",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                          {"AttributeName": "sk", "KeyType": "RANGE"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "lk", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "LocalSecondaryIndexes": [{
                "IndexName": "by-lk",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                              {"AttributeName": "lk", "KeyType": "RANGE"}],
                "Projection": {"ProjectionType": "INCLUDE"},
            }],
        }),
    )
    .await;
    assert_eq!(
        stated, INCLUDE_WITHOUT_ATTRIBUTES,
        "case {case:?}: expected the same sentence as for a global index, got {stated:?}"
    );

    // And for an index ADDED to a table that already exists.
    table(&router, "proj-update").await;
    let case = "an index added with an INCLUDE projection and no attributes";
    let stated = refusal(
        &router,
        case,
        "UpdateTable",
        json!({
            "TableName": "proj-update",
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "gk", "AttributeType": "S"},
            ],
            "GlobalSecondaryIndexUpdates": [{"Create": {
                "IndexName": "by-gk",
                "KeySchema": [{"AttributeName": "gk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "INCLUDE"},
            }}],
        }),
    )
    .await;
    assert_eq!(
        stated, INCLUDE_WITHOUT_ATTRIBUTES,
        "case {case:?}: expected the same sentence as for a create, got {stated:?}"
    );
}

// ===========================================================================
// 2. A refused create leaves no table behind
// ===========================================================================

#[tokio::test]
async fn a_table_a_refused_create_did_not_make_is_not_found() {
    let router = emulator();

    // Each row: the case and a create the service refuses for its own reasons.
    let refused: Vec<(&str, Value)> = vec![
        (
            "an INCLUDE projection with no attributes",
            create_with_index_projection("ghost-include", json!({"ProjectionType": "INCLUDE"})),
        ),
        (
            "a key schema naming one attribute twice",
            json!({
                "TableName": "ghost-duplicate",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                              {"AttributeName": "pk", "KeyType": "RANGE"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
        ),
        (
            "a stream left disabled with a view type named",
            json!({
                "TableName": "ghost-stream",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
                "StreamSpecification": {"StreamEnabled": false,
                                        "StreamViewType": "NEW_IMAGE"},
            }),
        ),
    ];

    for (case, request) in refused {
        let name = request["TableName"].as_str().expect("a name").to_string();
        refusal(&router, case, "CreateTable", request).await;

        // The reader reports the table as missing, with the name the request asked for.
        let stated = refusal_coded(
            &router,
            case,
            "DescribeTable",
            json!({"TableName": name.clone()}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            stated,
            format!("Requested resource not found: Table: {name} not found"),
            "case {case:?}: expected the not-found sentence over the requested name, got \
             {stated:?}"
        );

        // And no listing carries it either.
        let listed = ok(&router, "ListTables", json!({})).await;
        let names = listed["TableNames"].as_array().cloned().unwrap_or_default();
        assert!(
            !names.contains(&json!(name)),
            "case {case:?}: a refused create must leave no table in the listing, got {listed}"
        );
    }
}

// ===========================================================================
// 3. A type test names a type code
// ===========================================================================

/// Every member of the `AttributeValue` union, with one value of that type.
///
/// Source: this service's Smithy model, `AttributeValue`
/// (`smithy_query(query="shape",shape="AttributeValue")`) — its members are `S`, `N`,
/// `B`, `SS`, `NS`, `BS`, `M`, `L`, `NULL` and `BOOL`, which is exactly the set a type
/// test may name.
fn attribute_value_types() -> Vec<(&'static str, Value)> {
    vec![
        ("S", json!({"S": "text"})),
        ("N", json!({"N": "1"})),
        ("B", json!({"B": "dGhpcw=="})),
        ("SS", json!({"SS": ["a"]})),
        ("NS", json!({"NS": ["1"]})),
        ("BS", json!({"BS": ["dGhpcw=="]})),
        ("M", json!({"M": {"inner": {"S": "text"}}})),
        ("L", json!({"L": [{"S": "text"}]})),
        ("NULL", json!({"NULL": true})),
        ("BOOL", json!({"BOOL": true})),
    ]
}

#[tokio::test]
async fn a_type_test_names_a_type_code() {
    let router = emulator();
    table(&router, "TypeTests").await;
    for (code, value) in attribute_value_types() {
        ok(
            &router,
            "PutItem",
            json!({"TableName": "TypeTests",
                   "Item": {"pk": {"S": format!("typed-{code}")}, "v": value}}),
        )
        .await;
    }

    // Every code the union declares is a code a filter may test for: the request is
    // served and selects the one item whose attribute has that type.
    for (code, _) in attribute_value_types() {
        let answered = ok(
            &router,
            "Scan",
            json!({
                "TableName": "TypeTests",
                "FilterExpression": "attribute_type(v, :t)",
                "ExpressionAttributeValues": {":t": {"S": code}},
            }),
        )
        .await;
        assert_eq!(
            answered["Count"],
            json!(1),
            "case the {code} code: a type test on a declared code selects its own item, got \
             {answered}"
        );
    }

    // A token that is no code at all: spelled-out names, a lower-cased code, a code of
    // another system, and the empty token. None of them is a type, so none of them can
    // select anything, and the request is refused rather than answered with nothing.
    for token in ["STRING", "Number", "s", "BOOLEAN", "MAP", "INT", ""] {
        let case = format!("the token {token:?} on a Scan filter");
        let stated = refusal(
            &router,
            &case,
            "Scan",
            json!({
                "TableName": "TypeTests",
                "FilterExpression": "attribute_type(v, :t)",
                "ExpressionAttributeValues": {":t": {"S": token}},
            }),
        )
        .await;
        assert_eq!(
            stated,
            format!(
                "Invalid FilterExpression: Invalid attribute type name found; type: {token}, \
                 valid types: {{S,SS,N,NS,B,BS,BOOL,NULL,L,M}}"
            ),
            "case {case:?}: expected the type-name sentence over the whole code set, got \
             {stated:?}"
        );

        // The same token through the member a QUERY filters with, and through the member
        // a WRITE states its condition with — which carries the validation envelope.
        let case = format!("the token {token:?} on a Query filter");
        let stated = refusal(
            &router,
            &case,
            "Query",
            json!({
                "TableName": "TypeTests",
                "KeyConditionExpression": "pk = :k",
                "FilterExpression": "attribute_type(v, :t)",
                "ExpressionAttributeValues": {":k": {"S": "typed-S"}, ":t": {"S": token}},
            }),
        )
        .await;
        assert!(
            stated.starts_with("Invalid FilterExpression: Invalid attribute type name found"),
            "case {case:?}: expected the same sentence on a Query, got {stated:?}"
        );

        let case = format!("the token {token:?} on a write condition");
        let stated = refusal(
            &router,
            &case,
            "PutItem",
            json!({
                "TableName": "TypeTests",
                "Item": {"pk": {"S": "typed-S"}},
                "ConditionExpression": "attribute_type(v, :t)",
                "ExpressionAttributeValues": {":t": {"S": token}},
            }),
        )
        .await;
        assert_eq!(
            stated,
            format!(
                "1 validation error detected: Invalid ConditionExpression: Invalid attribute \
                 type name found; type: {token}, valid types: {{S,SS,N,NS,B,BS,BOOL,NULL,L,M}}"
            ),
            "case {case:?}: expected the write surface's enveloped form, got {stated:?}"
        );

        // And through a statement, which states the same fact in its own words.
        let case = format!("the token {token:?} in a statement");
        let stated = refusal(
            &router,
            &case,
            "ExecuteStatement",
            json!({
                "Statement": format!(
                    "SELECT * FROM TypeTests WHERE pk = 'typed-S' AND attribute_type(v, '{token}')"
                ),
            }),
        )
        .await;
        assert_eq!(
            stated,
            format!(
                "Invalid attribute type name found; type: {token}, valid types: \
                 {{N,BS,L,B,NULL,M,S,SS,NS,BOOL}}"
            ),
            "case {case:?}: expected the statement surface's form, got {stated:?}"
        );
    }

    // The second operand of a type test is a type NAME, so a value of another type is
    // refused as the operand type it is.
    let case = "a number where the type code belongs";
    let stated = refusal(
        &router,
        case,
        "Scan",
        json!({
            "TableName": "TypeTests",
            "FilterExpression": "attribute_type(v, :t)",
            "ExpressionAttributeValues": {":t": {"N": "1"}},
        }),
    )
    .await;
    assert_eq!(
        stated,
        "Invalid FilterExpression: Incorrect operand type for operator or function; operator or \
         function: attribute_type, operand type: N",
        "case {case:?}: expected the operand-type sentence, got {stated:?}"
    );
}

// ===========================================================================
// 4. An index a request adds declares its key attributes
// ===========================================================================

#[tokio::test]
async fn an_index_a_request_adds_declares_its_key_attributes() {
    let router = emulator();
    table(&router, "IndexDefs").await;

    // Each row: the case, the AttributeDefinitions the request declares, the key schema
    // of the index it creates, and whether the request holds together.
    let rows: Vec<(&str, Value, Value, bool)> = vec![
        (
            "no definitions at all",
            Value::Null,
            json!([{"AttributeName": "gk", "KeyType": "HASH"}]),
            false,
        ),
        (
            "definitions that leave the index key out",
            json!([{"AttributeName": "pk", "AttributeType": "S"}]),
            json!([{"AttributeName": "gk", "KeyType": "HASH"}]),
            false,
        ),
        (
            "definitions that cover the partition key but not the sort key",
            json!([{"AttributeName": "gk", "AttributeType": "S"}]),
            json!([{"AttributeName": "gk", "KeyType": "HASH"},
                   {"AttributeName": "sk", "KeyType": "RANGE"}]),
            false,
        ),
        (
            "definitions that cover every key attribute",
            json!([{"AttributeName": "gk", "AttributeType": "S"}]),
            json!([{"AttributeName": "gk", "KeyType": "HASH"}]),
            true,
        ),
    ];

    for (case, definitions, key_schema, holds) in rows {
        let index = format!("by-{}", case.replace(' ', "-"));
        // The two lists the refusal below names, read off the row before the request
        // takes ownership of them.
        let schema_names: Vec<Value> = key_schema.as_array().cloned().unwrap_or_default();
        let declared_names: Vec<String> = definitions
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|definition| {
                definition["AttributeName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let mut request = json!({
            "TableName": "IndexDefs",
            "GlobalSecondaryIndexUpdates": [{"Create": {
                "IndexName": index.clone(),
                "KeySchema": key_schema,
                "Projection": {"ProjectionType": "ALL"},
            }}],
        });
        if !definitions.is_null() {
            request["AttributeDefinitions"] = definitions;
        }
        if holds {
            ok(&router, "UpdateTable", request).await;
            continue;
        }
        let stated = refusal(&router, case, "UpdateTable", request).await;
        // AWS's sentence for a key attribute with no definition names BOTH lists: the
        // index's whole key schema, and the attributes the request declared.
        let keys: Vec<String> = schema_names
            .iter()
            .map(|element| element["AttributeName"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            stated,
            format!(
                "One or more parameter values were invalid: Some index key attributes are not \
                 defined in AttributeDefinitions. Keys: [{}], AttributeDefinitions: [{}]",
                keys.join(", "),
                declared_names.join(", ")
            ),
            "case {case:?}: expected the sentence naming both lists, got {stated:?}"
        );
    }
}

// ===========================================================================
// 5. A statement clause states one action
// ===========================================================================

/// AWS's fixed sentence for one token past a complete statement.
const UNEXPECTED_TOKEN: &str =
    "Statement wasn't well formed, can't be processed: Unexpected token after expression";

#[tokio::test]
async fn a_statement_clause_states_one_action() {
    let router = emulator();
    table(&router, "Clauses").await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Clauses", "Item": {"pk": {"S": "one"}, "a": {"N": "1"},
                                                "b": {"N": "2"}}}),
    )
    .await;

    // Each row: the case, the statement, and whether the dialect spells it.
    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "two assignments separated by a comma",
            "UPDATE Clauses SET a = 3, b = 4 WHERE pk = 'one'",
            false,
        ),
        (
            "two removals separated by a comma",
            "UPDATE Clauses REMOVE a, b WHERE pk = 'one'",
            false,
        ),
        (
            "a comma after the only assignment",
            "UPDATE Clauses SET a = 3, WHERE pk = 'one'",
            false,
        ),
        (
            "a surplus word after a complete select",
            "SELECT * FROM Clauses WHERE pk = 'one' surplus",
            false,
        ),
        (
            "one assignment per SET clause",
            "UPDATE Clauses SET a = 3 SET b = 4 WHERE pk = 'one'",
            true,
        ),
        (
            "a SET clause beside a REMOVE clause",
            "UPDATE Clauses SET a = 5 REMOVE b WHERE pk = 'one'",
            true,
        ),
        (
            "one removal per REMOVE clause",
            "UPDATE Clauses REMOVE a REMOVE b WHERE pk = 'one'",
            true,
        ),
    ];

    for (case, statement, spelled) in rows {
        if spelled {
            ok(&router, "ExecuteStatement", json!({"Statement": statement})).await;
            continue;
        }
        let stated = refusal(&router, case, "ExecuteStatement", json!({"Statement": statement}))
            .await;
        assert_eq!(
            stated, UNEXPECTED_TOKEN,
            "case {case:?}: expected the surplus-token sentence, got {stated:?}"
        );
    }
}

// ===========================================================================
// 6. A substitution table defines something
// ===========================================================================

#[tokio::test]
async fn a_substitution_table_defines_something() {
    let router = emulator();
    table(&router, "Substitutions").await;

    // Every operation that takes the two substitution tables, with the members it needs
    // to be otherwise well formed.
    let operations: Vec<(&str, Value)> = vec![
        (
            "UpdateItem",
            json!({"TableName": "Substitutions", "Key": {"pk": {"S": "one"}}}),
        ),
        (
            "PutItem",
            json!({"TableName": "Substitutions", "Item": {"pk": {"S": "one"}}}),
        ),
        (
            "DeleteItem",
            json!({"TableName": "Substitutions", "Key": {"pk": {"S": "one"}}}),
        ),
        ("Scan", json!({"TableName": "Substitutions"})),
        (
            "Query",
            json!({"TableName": "Substitutions",
                   "KeyConditionExpression": "pk = :k",
                   "ExpressionAttributeValues": {":k": {"S": "one"}}}),
        ),
    ];

    for (operation, base) in &operations {
        // An empty VALUES table, except where the operation needs that member to state
        // its key condition.
        if operation != &"Query" {
            let mut request = base.clone();
            request["ExpressionAttributeValues"] = json!({});
            let case = format!("an empty ExpressionAttributeValues on {operation}");
            let stated = refusal(&router, &case, operation, request).await;
            assert_eq!(
                stated,
                "1 validation error detected: ExpressionAttributeValues must not be empty",
                "case {case:?}: expected the empty-member sentence, got {stated:?}"
            );
        }

        // An empty NAMES table, on every one of them.
        let mut request = base.clone();
        request["ExpressionAttributeNames"] = json!({});
        let case = format!("an empty ExpressionAttributeNames on {operation}");
        let stated = refusal(&router, &case, operation, request).await;
        assert_eq!(
            stated,
            "1 validation error detected: ExpressionAttributeNames must not be empty",
            "case {case:?}: expected the empty-member sentence, got {stated:?}"
        );
    }

    // A names table that DEFINES something, where what it defines is no attribute name.
    for alias in ["#s", "#other"] {
        let case = format!("an empty attribute name under {alias}");
        let stated = refusal(
            &router,
            &case,
            "Scan",
            json!({
                "TableName": "Substitutions",
                "FilterExpression": format!("attribute_exists({alias})"),
                "ExpressionAttributeNames": {alias: ""},
            }),
        )
        .await;
        assert_eq!(
            stated,
            format!(
                "ExpressionAttributeNames contains invalid value: Empty attribute name for key \
                 {alias}"
            ),
            "case {case:?}: expected the sentence naming the key, got {stated:?}"
        );
    }

    // A table the request does not send at all is how an expression that needs no
    // substitution is spelled, so it is served.
    ok(&router, "Scan", json!({"TableName": "Substitutions"})).await;
}

// ===========================================================================
// 7. A time range runs forwards
// ===========================================================================

/// AWS's fixed text for a time range whose bounds are the wrong way round.
const INVERTED_RANGE: &str = "One or more parameter values were invalid: Time range lower bound \
                              must be less than or equal to upper bound";

#[tokio::test]
async fn a_time_range_runs_forwards() {
    let router = emulator();

    // Each row: the case, the lower bound, the upper bound, and whether the pair states
    // a window. The bounds span both orders, equality, a bound on its own and neither.
    let rows: Vec<(&str, Option<i64>, Option<i64>, bool)> = vec![
        ("a lower bound after the upper one", Some(2_000), Some(1_000), false),
        ("a lower bound one millisecond past it", Some(1_001), Some(1_000), false),
        ("the two bounds equal", Some(1_000), Some(1_000), true),
        ("a lower bound before the upper one", Some(1_000), Some(2_000), true),
        ("a lower bound on its own", Some(1_000), None, true),
        ("an upper bound on its own", None, Some(1_000), true),
        ("neither bound", None, None, true),
    ];

    for (case, lower, upper, states_a_window) in rows {
        let mut request = json!({});
        if let Some(lower) = lower {
            request["TimeRangeLowerBound"] = json!(lower);
        }
        if let Some(upper) = upper {
            request["TimeRangeUpperBound"] = json!(upper);
        }
        if states_a_window {
            let answered = ok(&router, "ListBackups", request).await;
            assert!(
                answered["BackupSummaries"].is_array(),
                "case {case:?}: a window that states a range is answered with a listing, got \
                 {answered}"
            );
            continue;
        }
        let stated = refusal(&router, case, "ListBackups", request).await;
        assert_eq!(
            stated, INVERTED_RANGE,
            "case {case:?}: expected the time-range sentence, got {stated:?}"
        );
    }
}

// ===========================================================================
// 8. Every value a request carries obeys the rules for its type
// ===========================================================================

#[tokio::test]
async fn every_value_a_request_carries_obeys_the_rules_for_its_type() {
    let router = emulator();
    table(&router, "Values").await;

    // Each row: the case, a value that breaks a documented rule for its type, and the
    // clause the service states for that rule. The rows span the numeric domain (the
    // text, the precision, the magnitude) and the set rules (emptiness, distinctness).
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a number of 39 significant digits",
            json!({"N": THIRTY_NINE_DIGITS}),
            "Attempting to store more than 38 significant digits in a Number".to_string(),
        ),
        (
            "a text that is no number",
            json!({"N": "abc"}),
            "The parameter cannot be converted to a numeric value: abc".to_string(),
        ),
        (
            "a magnitude past the documented exponent range",
            json!({"N": "1e130"}),
            "Number overflow. Attempting to store a number with magnitude larger than supported \
             range"
                .to_string(),
        ),
        (
            "a zero carrying an exponent past the documented range",
            json!({"N": "0e126"}),
            "Number overflow. Attempting to store a number with magnitude larger than supported \
             range"
                .to_string(),
        ),
        (
            "an empty string set",
            json!({"SS": []}),
            "One or more parameter values were invalid: An string set  may not be empty"
                .to_string(),
        ),
        (
            "a string set holding one member twice",
            json!({"SS": ["a", "a"]}),
            "One or more parameter values were invalid: Input collection [\"a\", \"a\"] contains \
             duplicates."
                .to_string(),
        ),
        (
            "a number nested inside a list",
            json!({"L": [{"N": THIRTY_NINE_DIGITS}]}),
            "Attempting to store more than 38 significant digits in a Number".to_string(),
        ),
    ];

    for (case, value, clause) in rows {
        // The value as an ITEM attribute: the write surface states its clauses inside
        // the validation envelope.
        let stated = refusal(
            &router,
            &format!("{case} in an item"),
            "PutItem",
            json!({"TableName": "Values", "Item": {"pk": {"S": "one"}, "v": value.clone()}}),
        )
        .await;
        assert_eq!(
            stated,
            format!("1 validation error detected: {clause}"),
            "case {case:?} in an item: expected the enveloped clause, got {stated:?}"
        );

        // The same value bound to a WRITE's condition: the same surface, the same form.
        let stated = refusal(
            &router,
            &format!("{case} in a write's expression values"),
            "PutItem",
            json!({
                "TableName": "Values",
                "Item": {"pk": {"S": "one"}},
                "ConditionExpression": "v = :v",
                "ExpressionAttributeValues": {":v": value.clone()},
            }),
        )
        .await;
        assert_eq!(
            stated,
            format!("1 validation error detected: {clause}"),
            "case {case:?} in a write's expression values: expected the enveloped clause, got \
             {stated:?}"
        );

        // The same value bound to a READ's filter: the same rule, stated on its own, the
        // way every other refusal about a read's expression members is.
        let stated = refusal(
            &router,
            &format!("{case} in a read's expression values"),
            "Scan",
            json!({
                "TableName": "Values",
                "FilterExpression": "v = :v",
                "ExpressionAttributeValues": {":v": value.clone()},
            }),
        )
        .await;
        assert_eq!(
            stated, clause,
            "case {case:?} in a read's expression values: expected the bare clause, got {stated:?}"
        );

        // And the same value as a STATEMENT's parameter.
        let stated = refusal(
            &router,
            &format!("{case} as a statement parameter"),
            "ExecuteStatement",
            json!({
                "Statement": "SELECT * FROM Values WHERE pk = ?",
                "Parameters": [value.clone()],
            }),
        )
        .await;
        assert_eq!(
            stated, clause,
            "case {case:?} as a statement parameter: expected the bare clause, got {stated:?}"
        );
    }

    // The documented precision itself is accepted, wherever it appears: the bound is a
    // ceiling, not a refusal of every long number.
    let thirty_eight = &THIRTY_NINE_DIGITS[..38];
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Values",
               "Item": {"pk": {"S": "ok"}, "v": {"N": thirty_eight}}}),
    )
    .await;
    ok(
        &router,
        "Scan",
        json!({
            "TableName": "Values",
            "FilterExpression": "v = :v",
            "ExpressionAttributeValues": {":v": {"N": thirty_eight}},
        }),
    )
    .await;
}

// ===========================================================================
// 9. An ARN addresses this endpoint's own region
// ===========================================================================

/// The region an ARN names that is NOT the one these requests are signed for.
const OTHER_REGION: &str = "eu-west-1";

#[tokio::test]
async fn an_arn_addresses_this_endpoints_own_region() {
    let router = emulator();
    table(&router, "Addressed").await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Addressed", "Item": {"pk": {"S": "one"}}}),
    )
    .await;

    let arn_in = |region: &str| {
        format!("arn:aws:dynamodb:{region}:000000000000:table/Addressed")
    };

    // Each row: the operation and the rest of its request, over the data plane and the
    // control plane — every surface whose `TableName` takes an ARN in place of a name.
    let operations: Vec<(&str, Value)> = vec![
        ("GetItem", json!({"Key": {"pk": {"S": "one"}}})),
        ("PutItem", json!({"Item": {"pk": {"S": "two"}}})),
        ("DeleteItem", json!({"Key": {"pk": {"S": "one"}}})),
        ("Scan", json!({})),
        (
            "Query",
            json!({"KeyConditionExpression": "pk = :k",
                   "ExpressionAttributeValues": {":k": {"S": "one"}}}),
        ),
        ("DescribeTable", json!({})),
        ("ListBackups", json!({})),
    ];

    for (operation, rest) in &operations {
        // The ARN of ANOTHER region names a table this endpoint cannot reach.
        let mut request = rest.clone();
        request["TableName"] = json!(arn_in(OTHER_REGION));
        let case = format!("{operation} addressed by an ARN of {OTHER_REGION}");
        let stated = refusal(&router, &case, operation, request).await;
        assert_eq!(
            stated,
            format!(
                "1 validation error detected: Invalid AWS region in '{}'",
                arn_in(OTHER_REGION)
            ),
            "case {case:?}: expected the region sentence over the ARN, got {stated:?}"
        );

        // The ARN of THIS region names the same table the bare name does.
        let mut request = rest.clone();
        request["TableName"] = json!(arn_in(REGION));
        ok(&router, operation, request).await;

        // And so does the bare name.
        let mut request = rest.clone();
        request["TableName"] = json!("Addressed");
        ok(&router, operation, request).await;
    }

    // The rule is about the endpoint the request REACHED, not about one region: the same
    // ARN that is refused here is the one that resolves there.
    create_active_table(&router, "Elsewhere", &[("pk", "HASH", "S")]).await;
    let (status, answered) = call_in(
        &router,
        OTHER_REGION,
        "DescribeTable",
        json!({"TableName": format!("arn:aws:dynamodb:{REGION}:000000000000:table/Elsewhere")}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case an ARN of {REGION} at the {OTHER_REGION} endpoint: expected the same refusal, got \
         {status} with {answered}"
    );
}

// ===========================================================================
// 10. A configuration change changes something
// ===========================================================================

#[tokio::test]
async fn a_configuration_change_changes_something() {
    let router = emulator();
    table(&router, "Precision").await;
    // The destination's stream has to EXIST for the registration to replicate: one whose
    // `StreamArn` resolves to no stream settles `ENABLE_FAILED`, which no configuration
    // change can then address.
    let stream = crate::emulator::ensure_kinesis_stream(&router, "precision").await;
    let stream = stream.as_str();

    // Each row: the precision in force before the command and the precision it asks
    // for. The two values are the ones the model declares for
    // `ApproximateCreationDateTimePrecision`
    // (`smithy_query(query="shape",shape="ApproximateCreationDateTimePrecision")`).
    ok(
        &router,
        "EnableKinesisStreamingDestination",
        json!({
            "TableName": "Precision",
            "StreamArn": stream,
            "EnableKinesisStreamingConfiguration": {
                "ApproximateCreationDateTimePrecision": "MILLISECOND"
            },
        }),
    )
    .await;
    let described = crate::emulator::describe_when_active(&router, "precision", "Precision").await;
    assert_eq!(
        described["TableStatus"],
        json!("ACTIVE"),
        "case precision: the table settles before the destination is changed, got {described}"
    );
    // Wait out the destination's own transition, which a change requires.
    tokio::time::sleep(crate::emulator::ACTIVE_DEADLINE / 2).await;

    let table_id = described["TableId"].as_str().unwrap_or_default().to_string();
    let case = "the precision already in force";
    let stated = refusal(
        &router,
        case,
        "UpdateKinesisStreamingDestination",
        json!({
            "TableName": "Precision",
            "StreamArn": stream,
            "UpdateKinesisStreamingConfiguration": {
                "ApproximateCreationDateTimePrecision": "MILLISECOND"
            },
        }),
    )
    .await;
    assert_eq!(
        stated,
        format!(
            "Invalid Request: Precision is already set to the desired value of MILLISECOND for \
             tableId: {table_id}, kdsArn: {stream}"
        ),
        "case {case:?}: expected the sentence over the stored identity, got {stated:?}"
    );

    // The OTHER value is a change, so it is applied.
    let answered = ok(
        &router,
        "UpdateKinesisStreamingDestination",
        json!({
            "TableName": "Precision",
            "StreamArn": stream,
            "UpdateKinesisStreamingConfiguration": {
                "ApproximateCreationDateTimePrecision": "MICROSECOND"
            },
        }),
    )
    .await;
    assert_eq!(
        answered["UpdateKinesisStreamingConfiguration"]
            ["ApproximateCreationDateTimePrecision"],
        json!("MICROSECOND"),
        "case a precision that differs: the change is applied, got {answered}"
    );
}

// ===========================================================================
// 11. An expression names attributes rather than literals
// ===========================================================================

#[tokio::test]
async fn an_expression_names_attributes_rather_than_literals() {
    let router = emulator();
    table(&router, "Literals").await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Literals", "Item": {"pk": {"S": "one"}, "n": {"N": "1"},
                                                 "s": {"S": "x"}}}),
    )
    .await;

    // Each row: the filter, the token the parse stops at, and the text around it. Every
    // value an expression compares against is a `:placeholder`, so a bare numeric
    // literal is a token the grammar has no place for.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("n > 0", "0", "> 0"),
        ("n = 1", "1", "= 1"),
        ("size(s) > 0", "0", "> 0"),
        ("n BETWEEN 0 AND 2", "0", "BETWEEN 0 AND"),
        ("n2 = 1", "1", "= 1"),
    ];

    for (filter, token, near) in rows {
        let case = format!("the filter {filter:?}");
        let stated = refusal(
            &router,
            &case,
            "Scan",
            json!({"TableName": "Literals", "FilterExpression": filter}),
        )
        .await;
        assert_eq!(
            stated,
            format!("Invalid FilterExpression: Syntax error; token: \"{token}\", near: \"{near}\""),
            "case {case:?}: expected the syntax error naming the literal, got {stated:?}"
        );
    }

    // A name that merely CONTAINS digits is a name, and a placeholder is how a value is
    // spelled: both are served.
    ok(
        &router,
        "Scan",
        json!({
            "TableName": "Literals",
            "FilterExpression": "n > :zero",
            "ExpressionAttributeValues": {":zero": {"N": "0"}},
        }),
    )
    .await;
    ok(
        &router,
        "Scan",
        json!({"TableName": "Literals", "FilterExpression": "attribute_not_exists(n2)"}),
    )
    .await;
}

// ===========================================================================
// 12. A request states its intent one way
// ===========================================================================

#[tokio::test]
async fn a_request_states_its_intent_one_way() {
    let router = emulator();
    table(&router, "Generations").await;

    // Each row: the operation, the request, the non-expression member it carries, the
    // expression member that REPLACED that member, and whether the refusal carries the
    // validation envelope. A request that states one thing both ways is refused, naming
    // both whole sides; the envelope follows the members, so a WRITE stating its update
    // or its condition twice is refused inside it and a READ stating its projection, its
    // filter or its key condition twice is refused with the sentence alone.
    let rows: Vec<(&str, Value, &str, &str, bool)> = vec![
        (
            "Scan",
            json!({"TableName": "Generations", "AttributesToGet": ["a"],
                   "ProjectionExpression": "a"}),
            "AttributesToGet",
            "ProjectionExpression",
            false,
        ),
        (
            "Scan",
            json!({"TableName": "Generations",
                   "ScanFilter": {"a": {"ComparisonOperator": "NOT_NULL"}},
                   "FilterExpression": "attribute_exists(a)"}),
            "ScanFilter",
            "FilterExpression",
            false,
        ),
        (
            "Query",
            json!({"TableName": "Generations",
                   "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                            "AttributeValueList": [{"S": "one"}]}},
                   "KeyConditionExpression": "pk = :k",
                   "ExpressionAttributeValues": {":k": {"S": "one"}}}),
            "KeyConditions",
            "KeyConditionExpression",
            false,
        ),
        (
            "GetItem",
            json!({"TableName": "Generations", "Key": {"pk": {"S": "one"}},
                   "AttributesToGet": ["a"], "ProjectionExpression": "a"}),
            "AttributesToGet",
            "ProjectionExpression",
            false,
        ),
        (
            "PutItem",
            json!({"TableName": "Generations", "Item": {"pk": {"S": "one"}},
                   "Expected": {"a": {"Exists": false}},
                   "ConditionExpression": "attribute_not_exists(a)"}),
            "Expected",
            "ConditionExpression",
            true,
        ),
        (
            "UpdateItem",
            json!({"TableName": "Generations", "Key": {"pk": {"S": "one"}},
                   "AttributeUpdates": {"a": {"Action": "PUT", "Value": {"S": "x"}}},
                   "UpdateExpression": "SET a = :v",
                   "ExpressionAttributeValues": {":v": {"S": "x"}}}),
            "AttributeUpdates",
            "UpdateExpression",
            true,
        ),
        (
            "DeleteItem",
            json!({"TableName": "Generations", "Key": {"pk": {"S": "one"}},
                   "Expected": {"a": {"Exists": false}},
                   "ConditionExpression": "attribute_not_exists(a)"}),
            "Expected",
            "ConditionExpression",
            true,
        ),
    ];

    for (operation, request, legacy, expression, enveloped) in rows {
        let case = format!("{operation} carrying {legacy} and {expression}");
        let stated = refusal(&router, &case, operation, request).await;
        let envelope = if enveloped {
            "1 validation error detected: "
        } else {
            ""
        };
        assert_eq!(
            stated,
            format!(
                "{envelope}Can not use both expression and non-expression parameters in the same \
                 request: Non-expression parameters: {{{legacy}}} Expression parameters: \
                 {{{expression}}}"
            ),
            "case {case:?}: expected both sides to be named, got {stated:?}"
        );
    }

    // A legacy member beside an expression member that states something ELSE is refused
    // just the same: it is the two GENERATIONS a request may not mix, not a legacy member
    // beside the one that replaced it, so the legacy projection beside a filter
    // expression — and the legacy filter beside a projection expression — collide too,
    // each naming the whole of both sides.
    for (case, request, legacy, expression) in [
        (
            "the legacy projection beside a filter expression",
            json!({"TableName": "Generations", "AttributesToGet": ["a"],
                   "FilterExpression": "attribute_exists(a)"}),
            "AttributesToGet",
            "FilterExpression",
        ),
        (
            "the legacy filter beside a projection expression",
            json!({"TableName": "Generations",
                   "ScanFilter": {"a": {"ComparisonOperator": "NOT_NULL"}},
                   "ProjectionExpression": "a"}),
            "ScanFilter",
            "ProjectionExpression",
        ),
    ] {
        let stated = refusal(&router, case, "Scan", request).await;
        assert_eq!(
            stated,
            format!(
                "Can not use both expression and non-expression parameters in the same request: \
                 Non-expression parameters: {{{legacy}}} Expression parameters: {{{expression}}}"
            ),
            "case {case:?}: expected both sides to be named, got {stated:?}"
        );
    }

    // A request that stays inside ONE generation is served, whichever it picked: what is
    // refused is carrying members of both.
    for (case, request) in [
        (
            "both the projection and the filter stated the legacy way",
            json!({"TableName": "Generations", "AttributesToGet": ["a"],
                   "ScanFilter": {"a": {"ComparisonOperator": "NOT_NULL"}}}),
        ),
        (
            "both stated the expression way",
            json!({"TableName": "Generations", "ProjectionExpression": "a",
                   "FilterExpression": "attribute_exists(a)"}),
        ),
    ] {
        let (status, answered) = call(&router, "Scan", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a request inside one generation is served, got {status} with \
             {answered}"
        );
    }
}

// ===========================================================================
// 13. An Expected entry states what it expects
// ===========================================================================

#[tokio::test]
async fn an_expected_entry_states_what_it_expects() {
    let router = emulator();
    table(&router, "Expectations").await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Expectations", "Item": {"pk": {"S": "one"}, "attr": {"N": "1"}}}),
    )
    .await;

    // Each row: the case, the entry, and the clause the service states — `None` where
    // the entry states a condition the service can decide.
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "an entry stating neither Value nor Exists",
            json!({}),
            Some("Value must be provided when Exists is null for Attribute: attr"),
        ),
        (
            "Exists true with no Value",
            json!({"Exists": true}),
            Some("Value must be provided when Exists is true for Attribute: attr"),
        ),
        (
            "Value beside Exists false",
            json!({"Value": {"N": "1"}, "Exists": false}),
            Some("Value cannot be used when Exists is false for Attribute: attr"),
        ),
        (
            "Exists beside a ComparisonOperator",
            json!({"Exists": true, "ComparisonOperator": "EQ",
                   "AttributeValueList": [{"N": "1"}]}),
            Some("Exists and ComparisonOperator cannot be used together for Attribute: attr"),
        ),
        (
            "Value beside an AttributeValueList",
            json!({"Value": {"N": "1"}, "ComparisonOperator": "EQ",
                   "AttributeValueList": [{"N": "1"}]}),
            Some("Value and AttributeValueList cannot be used together for Attribute: attr"),
        ),
        ("a Value on its own", json!({"Value": {"N": "1"}}), None),
        ("Exists false on its own", json!({"Exists": false}), None),
        (
            "a comparison on its own",
            json!({"ComparisonOperator": "EQ", "AttributeValueList": [{"N": "1"}]}),
            None,
        ),
    ];

    for (case, entry, clause) in rows {
        let request = json!({
            "TableName": "Expectations",
            "Key": {"pk": {"S": "one"}},
            "Expected": {"attr": entry},
        });
        let Some(clause) = clause else {
            // A condition the service can decide is answered — either the update or the
            // conditional-check failure, never a validation refusal.
            let (status, answered) = call(&router, "UpdateItem", request).await;
            assert!(
                status == 200 || error_code(&answered) == "ConditionalCheckFailedException",
                "case {case:?}: expected the condition to be decided, got {status} with {answered}"
            );
            continue;
        };
        let stated = refusal(&router, case, "UpdateItem", request).await;
        assert_eq!(
            stated,
            format!("1 validation error detected: One or more parameter values were invalid: {clause}"),
            "case {case:?}: expected the enveloped clause, got {stated:?}"
        );
    }
}
