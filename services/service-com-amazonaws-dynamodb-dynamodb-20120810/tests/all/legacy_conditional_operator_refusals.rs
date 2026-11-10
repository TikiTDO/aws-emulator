// SPDX-License-Identifier: CC0-1.0

//! How the legacy condition parameters are refused: the `ConditionalOperator` member's
//! own value set, the map it needs beside it, and the message that names both
//! generations of parameter when a request mixes them.
//!
//! Three rules are pinned here, each over a TABLE spanning what it generalises over
//! rather than one recorded request:
//!
//! * a value outside `ConditionalOperator`'s set is refused with the REQUIREMENT ALONE —
//!   the service checks this member itself, so the clause carries neither the rejected
//!   value nor the member path, and it states the set the service publishes for it. The
//!   rows vary the rejected value's form and the operation that carried it, and one row
//!   sends a second defect in the same request to show which one is reported;
//! * `ConditionalOperator` combines the ENTRIES of the legacy map beside it, so the
//!   sentence a request earns follows how many entries that map holds: none, exactly
//!   one, or enough to combine (which is served). The rows vary the operation, which map
//!   the member would have combined, and the entry count;
//! * the mixed-generation message lists each side in the order the operation's INPUT
//!   SHAPE declares its members, which the rows vary by carrying different subsets of
//!   both sides on both readers.
//!
//! Which FORM a refusal takes follows the surface: a read states the sentence bare, and a
//! write states it inside the `<N> validation error(s) detected: ` envelope. Everything
//! is driven over the wire, so nothing is asserted that a client could not observe.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table every row below drives: a composite key, so a `Query` can state a key
/// condition, plus the non-key attributes the filters name.
const TABLE: &str = "LegacyConditions";

/// The envelope a write's refusal carries, and a read's does not.
const ENVELOPE: &str = "1 validation error detected: ";

/// The set the service states for `ConditionalOperator`.
///
/// It is the SERVICE's own set, not the model's: the model declares `AND OR`
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ConditionalOperator")`)
/// while the refusal states `ALL` for the value `AND` spells.
const CONDITIONAL_OPERATOR_SET: &str = "[ALL, OR]";

/// The clause a value outside that set earns, with no value echoed and no member path.
fn requirement_clause() -> String {
    format!(
        "Failed to satisfy constraint: Member must satisfy enum value set: \
         {CONDITIONAL_OPERATOR_SET}"
    )
}

/// An emulator holding one active table and one item.
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
    describe_when_active(&router, "the legacy-conditions table", TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": TABLE,
            "Item": {"pk": {"S": "k1"}, "sk": {"S": "s1"}, "a": {"S": "x"}, "b": {"S": "y"}},
        }),
    )
    .await;
    router
}

/// One surface the member arrives on: the operation, a request carrying the operator the
/// row rejects, and the envelope that surface states its clause inside.
type Surface = (&'static str, fn(&str) -> Value, &'static str);

#[tokio::test]
async fn a_conditional_operator_outside_its_set_states_the_requirement_alone() {
    let router = seeded().await;

    // The rows vary the FORM of the rejected value (a misspelling, a declared value in
    // the wrong case, the empty string, a value of another enum, and the value the
    // published set spells that the model does not declare) and the operation that
    // carried it — the sentence is the same for all of them, and only the envelope
    // follows the surface.
    let values = ["XOR", "and", "", "ALL_OLD", "ALL"];
    let surfaces: [Surface; 5] = [
        (
            "Scan",
            |operator| {
                json!({
                    "TableName": TABLE,
                    "ConditionalOperator": operator,
                    "ScanFilter": {
                        "a": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "x"}]},
                        "b": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "y"}]},
                    },
                })
            },
            "",
        ),
        (
            "Query",
            |operator| {
                json!({
                    "TableName": TABLE,
                    "ConditionalOperator": operator,
                    "KeyConditions": {
                        "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                    },
                    "QueryFilter": {
                        "a": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "x"}]},
                        "b": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "y"}]},
                    },
                })
            },
            "",
        ),
        (
            "PutItem",
            |operator| {
                json!({
                    "TableName": TABLE,
                    "Item": {"pk": {"S": "k2"}, "sk": {"S": "s2"}},
                    "ConditionalOperator": operator,
                    "Expected": {
                        "a": {"Exists": false},
                        "b": {"Exists": false},
                    },
                })
            },
            ENVELOPE,
        ),
        (
            "DeleteItem",
            |operator| {
                json!({
                    "TableName": TABLE,
                    "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                    "ConditionalOperator": operator,
                    "Expected": {
                        "a": {"Exists": false},
                        "b": {"Exists": false},
                    },
                })
            },
            ENVELOPE,
        ),
        (
            "UpdateItem",
            |operator| {
                json!({
                    "TableName": TABLE,
                    "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                    "AttributeUpdates": {"c": {"Action": "PUT", "Value": {"S": "z"}}},
                    "ConditionalOperator": operator,
                    "Expected": {
                        "a": {"Exists": false},
                        "b": {"Exists": false},
                    },
                })
            },
            ENVELOPE,
        ),
    ];

    for (operation, request, envelope) in surfaces {
        for value in values {
            let case = format!("{operation} carrying the operator {value:?}");
            let (status, answered) = call(&router, operation, request(value)).await;
            let expected = format!("{envelope}{}", requirement_clause());
            assert_eq!(
                (status, error_code(&answered).as_str(), message(&answered)),
                (400, "ValidationException", expected.clone()),
                "case {case:?}: expected {expected:?}, got {status} with {answered}"
            );
        }
    }
}

#[tokio::test]
async fn the_conditional_operator_defect_is_reported_before_the_handler_reads_the_map() {
    let router = seeded().await;

    // Each request carries TWO defects: an operator outside the set, which the request
    // walk finds, and a legacy entry whose `AttributeValueList` holds the wrong number of
    // operands for the comparison it names, which the handler finds when it reads the
    // map. The rows differ in the operation and in which map carries the second defect;
    // the operator is reported either way, because a defect of the request's own members
    // precedes every rule about what those members mean.
    let both_defects: [(&str, &str, Value, &str); 2] = [
        (
            "a Scan whose ScanFilter also miscounts its operands",
            "Scan",
            json!({
                "TableName": TABLE,
                "ConditionalOperator": "XOR",
                "ScanFilter": {
                    "a": {"ComparisonOperator": "BETWEEN", "AttributeValueList": [{"S": "x"}]},
                    "b": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "y"}]},
                },
            }),
            "",
        ),
        (
            "a PutItem whose Expected also miscounts its operands",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k3"}, "sk": {"S": "s3"}},
                "ConditionalOperator": "XOR",
                "Expected": {
                    "a": {"ComparisonOperator": "BETWEEN", "AttributeValueList": [{"S": "x"}]},
                    "b": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "y"}]},
                },
            }),
            ENVELOPE,
        ),
    ];

    for (case, operation, request, envelope) in both_defects {
        let expected = format!("{envelope}{}", requirement_clause());
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected {expected:?}, got {status} with {answered}"
        );
    }

    // The second defect ALONE is reported as itself, so the rows above report the
    // operator because the operator comes first and not because the operand count goes
    // unchecked.
    let (status, answered) = call(
        &router,
        "Scan",
        json!({
            "TableName": TABLE,
            "ScanFilter": {
                "a": {"ComparisonOperator": "BETWEEN", "AttributeValueList": [{"S": "x"}]},
                "b": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "y"}]},
            },
        }),
    )
    .await;
    let operands = "One or more parameter values were invalid: Invalid number of argument(s) for \
                    the BETWEEN ComparisonOperator";
    assert_eq!(
        (status, error_code(&answered).as_str(), message(&answered)),
        (400, "ValidationException", operands.to_string()),
        "case the operand count alone: expected {operands:?}, got {status} with {answered}"
    );
}

#[tokio::test]
async fn another_enumerated_member_keeps_the_generic_constraint_clause() {
    let router = seeded().await;

    // The contrast the rule above is defined against: a member the generic request walk
    // checks is reported with the value echoed and the member path named, inside the
    // envelope, on the very operations whose `ConditionalOperator` refusal is bare. The
    // sets are the model's own declaration order
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ReturnConsumedCapacity")`
    // and `…#Select`).
    for (case, operation, request, clause) in [
        (
            "a Scan's ReturnConsumedCapacity",
            "Scan",
            json!({"TableName": TABLE, "ReturnConsumedCapacity": "NOPE"}),
            "Value 'NOPE' at 'returnConsumedCapacity' failed to satisfy constraint: Member must \
             satisfy enum value set: [INDEXES, TOTAL, NONE]",
        ),
        (
            "a Query's Select",
            "Query",
            json!({
                "TableName": TABLE,
                "Select": "EVERYTHING",
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
            }),
            "Value 'EVERYTHING' at 'select' failed to satisfy constraint: Member must satisfy \
             enum value set: [ALL_ATTRIBUTES, ALL_PROJECTED_ATTRIBUTES, SPECIFIC_ATTRIBUTES, \
             COUNT]",
        ),
    ] {
        let expected = format!("{ENVELOPE}{clause}");
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected {expected:?}, got {status} with {answered}"
        );
    }
}

/// The two sentences the companion-map rule states, selected on the entry count.
const WITHOUT_MAP: &str = "ConditionalOperator cannot be used without Filter or Expected";
const NEEDS_TWO: &str =
    "ConditionalOperator can only be used when Filter or Expected has two or more elements";

/// One `Condition`/`ExpectedAttributeValue` entry per attribute named, as the legacy maps
/// spell one.
fn filter_entries(names: &[&str]) -> Value {
    let mut map = serde_json::Map::new();
    for name in names {
        map.insert(
            (*name).to_string(),
            json!({"ComparisonOperator": "NOT_NULL"}),
        );
    }
    Value::Object(map)
}

/// One `ExpectedAttributeValue` entry per attribute named, each asking that the
/// attribute be ABSENT — a condition the keys the success rows address satisfy, so a row
/// that is served is served because the map held entries to combine.
fn expected_entries(names: &[&str]) -> Value {
    let mut map = serde_json::Map::new();
    for name in names {
        map.insert((*name).to_string(), json!({"Exists": false}));
    }
    Value::Object(map)
}

#[tokio::test]
async fn a_conditional_operator_is_refused_by_the_count_of_the_map_it_combines() {
    let router = seeded().await;

    // Each row: the case, the operation, the request, and the message it must earn —
    // `None` where the request is served because the map holds entries to combine. The
    // rows span both readers and all three writes, the three entry counts (none, the
    // member absent altogether, exactly one, and two), and the value of the operator
    // itself, which the rule is indifferent to.
    let rows: Vec<(&str, &str, Value, Option<String>)> = vec![
        (
            "a Scan whose ScanFilter is an empty map",
            "Scan",
            json!({"TableName": TABLE, "ConditionalOperator": "AND", "ScanFilter": {}}),
            Some(WITHOUT_MAP.to_string()),
        ),
        (
            "a Scan carrying no ScanFilter at all",
            "Scan",
            json!({"TableName": TABLE, "ConditionalOperator": "OR"}),
            Some(WITHOUT_MAP.to_string()),
        ),
        (
            "a Scan carrying only AttributesToGet",
            "Scan",
            json!({
                "TableName": TABLE,
                "ConditionalOperator": "AND",
                "AttributesToGet": ["a"],
            }),
            Some(WITHOUT_MAP.to_string()),
        ),
        (
            "a Scan whose ScanFilter holds one entry",
            "Scan",
            json!({
                "TableName": TABLE,
                "ConditionalOperator": "AND",
                "ScanFilter": filter_entries(&["a"]),
            }),
            Some(NEEDS_TWO.to_string()),
        ),
        (
            "a Scan whose ScanFilter holds two entries",
            "Scan",
            json!({
                "TableName": TABLE,
                "ConditionalOperator": "OR",
                "ScanFilter": filter_entries(&["a", "b"]),
            }),
            None,
        ),
        (
            "a Query carrying no key condition at all",
            "Query",
            json!({"TableName": TABLE, "ConditionalOperator": "AND"}),
            Some(WITHOUT_MAP.to_string()),
        ),
        (
            "a Query carrying no QueryFilter",
            "Query",
            json!({
                "TableName": TABLE,
                "ConditionalOperator": "AND",
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
            }),
            Some(WITHOUT_MAP.to_string()),
        ),
        (
            "a Query whose QueryFilter holds one entry",
            "Query",
            json!({
                "TableName": TABLE,
                "ConditionalOperator": "OR",
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "QueryFilter": filter_entries(&["a"]),
            }),
            Some(NEEDS_TWO.to_string()),
        ),
        (
            "a Query whose QueryFilter holds two entries",
            "Query",
            json!({
                "TableName": TABLE,
                "ConditionalOperator": "AND",
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "QueryFilter": filter_entries(&["a", "b"]),
            }),
            None,
        ),
        (
            "a PutItem carrying no Expected",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k4"}, "sk": {"S": "s4"}},
                "ConditionalOperator": "AND",
            }),
            Some(format!("{ENVELOPE}{WITHOUT_MAP}")),
        ),
        (
            "a PutItem whose Expected is an empty map",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k4"}, "sk": {"S": "s4"}},
                "ConditionalOperator": "OR",
                "Expected": {},
            }),
            Some(format!("{ENVELOPE}{WITHOUT_MAP}")),
        ),
        (
            "a PutItem whose Expected holds one entry",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k4"}, "sk": {"S": "s4"}},
                "ConditionalOperator": "AND",
                "Expected": expected_entries(&["a"]),
            }),
            Some(format!("{ENVELOPE}{NEEDS_TWO}")),
        ),
        (
            "a PutItem whose Expected holds two entries",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k4"}, "sk": {"S": "s4"}},
                "ConditionalOperator": "AND",
                "Expected": expected_entries(&["a", "b"]),
            }),
            None,
        ),
        (
            "a DeleteItem carrying no Expected",
            "DeleteItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k9"}, "sk": {"S": "s9"}},
                "ConditionalOperator": "OR",
            }),
            Some(format!("{ENVELOPE}{WITHOUT_MAP}")),
        ),
        (
            "a DeleteItem whose Expected holds one entry",
            "DeleteItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k9"}, "sk": {"S": "s9"}},
                "ConditionalOperator": "AND",
                "Expected": expected_entries(&["a"]),
            }),
            Some(format!("{ENVELOPE}{NEEDS_TWO}")),
        ),
        (
            "a DeleteItem whose Expected holds two entries",
            "DeleteItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k9"}, "sk": {"S": "s9"}},
                "ConditionalOperator": "AND",
                "Expected": expected_entries(&["a", "b"]),
            }),
            None,
        ),
        (
            "an UpdateItem carrying no Expected",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "AttributeUpdates": {"c": {"Action": "PUT", "Value": {"S": "z"}}},
                "ConditionalOperator": "AND",
            }),
            Some(format!("{ENVELOPE}{WITHOUT_MAP}")),
        ),
        (
            "an UpdateItem whose Expected holds one entry",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "AttributeUpdates": {"c": {"Action": "PUT", "Value": {"S": "z"}}},
                "ConditionalOperator": "OR",
                "Expected": expected_entries(&["a"]),
            }),
            Some(format!("{ENVELOPE}{NEEDS_TWO}")),
        ),
        (
            "an UpdateItem whose Expected holds two entries",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k8"}, "sk": {"S": "s8"}},
                "AttributeUpdates": {"c": {"Action": "PUT", "Value": {"S": "z"}}},
                "ConditionalOperator": "AND",
                "Expected": expected_entries(&["a", "b"]),
            }),
            None,
        ),
    ];

    for (case, operation, request, refusal) in rows {
        let (status, answered) = call(&router, operation, request).await;
        match refusal {
            Some(expected) => assert_eq!(
                (status, error_code(&answered).as_str(), message(&answered)),
                (400, "ValidationException", expected.clone()),
                "case {case:?}: expected {expected:?}, got {status} with {answered}"
            ),
            None => assert_eq!(
                status, 200,
                "case {case:?}: a map with entries to combine is served, got {status} with \
                 {answered}"
            ),
        }
    }
}

#[tokio::test]
async fn a_value_a_legacy_condition_compares_against_is_held_to_the_value_rules() {
    let router = seeded().await;

    /// A value nested `levels` deep: a map inside a map inside …, past the documented 32
    /// levels ("DynamoDB supports nested attributes up to 32 levels deep" — the item
    /// quotas).
    fn nested_value(levels: usize) -> Value {
        let mut value = json!({"S": "deep"});
        for _ in 0..levels {
            value = json!({"M": {"n": value}});
        }
        value
    }

    let nesting = "Nesting Levels have exceeded supported limits: Attributes in the item have \
                   nested levels beyond supported limit";
    // The rules belong to the VALUE, not to the surface it arrived through: a value a
    // legacy condition compares against is held to them exactly as a value a write
    // stores is. The rows vary the surface (both readers' filter maps and a write's
    // `Expected`), the member the value sits in (`AttributeValueList` or `Value`), the
    // rule it breaks (the documented nesting depth, and the numeric domain) and the depth
    // itself.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a ScanFilter operand nested 40 levels",
            "Scan",
            json!({
                "TableName": TABLE,
                "ScanFilter": {
                    "a": {"ComparisonOperator": "EQ", "AttributeValueList": [nested_value(40)]},
                },
            }),
            nesting.to_string(),
        ),
        (
            "a QueryFilter operand nested 50 levels",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "QueryFilter": {
                    "a": {"ComparisonOperator": "EQ", "AttributeValueList": [nested_value(50)]},
                },
            }),
            nesting.to_string(),
        ),
        (
            "a ScanFilter operand whose number is outside the documented range",
            "Scan",
            json!({
                "TableName": TABLE,
                "ScanFilter": {
                    "a": {"ComparisonOperator": "EQ", "AttributeValueList": [{"N": "1e130"}]},
                },
            }),
            "Number overflow. Attempting to store a number with magnitude larger than supported \
             range"
                .to_string(),
        ),
        (
            "an Expected AttributeValueList operand nested 40 levels",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k6"}, "sk": {"S": "s6"}},
                "Expected": {
                    "a": {"ComparisonOperator": "EQ", "AttributeValueList": [nested_value(40)]},
                },
            }),
            format!("{ENVELOPE}{nesting}"),
        ),
        (
            "an Expected Value nested 40 levels",
            "DeleteItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "Expected": {"a": {"Value": nested_value(40)}},
            }),
            format!("{ENVELOPE}{nesting}"),
        ),
        (
            "an Expected Value whose number is outside the documented range",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "AttributeUpdates": {"c": {"Action": "PUT", "Value": {"S": "z"}}},
                "Expected": {"a": {"Value": {"N": "1e-140"}}},
            }),
            format!(
                "{ENVELOPE}Number underflow. Attempting to store a number with magnitude smaller \
                 than supported range"
            ),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected {expected:?}, got {status} with {answered}"
        );
    }

    // A value INSIDE the rules is compared, not refused: the rows above are the rule
    // being broken rather than the member being rejected.
    let served = ok(
        &router,
        "Scan",
        json!({
            "TableName": TABLE,
            "ScanFilter": {
                "a": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "x"}]},
            },
        }),
    )
    .await;
    assert_eq!(
        served["Count"].clone(),
        json!(1),
        "case a filter inside the rules: expected the one matching item, got {served}"
    );
}

#[tokio::test]
async fn a_batch_read_that_mixes_the_two_generations_across_its_entries_is_refused() {
    let router = seeded().await;
    // A second table, so the two generations can sit in two DIFFERENT entries of one
    // request: the rule is about the REQUEST, so it holds across its entries exactly as
    // it holds inside one.
    let other = "LegacyConditionsToo";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": other,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the second legacy-conditions table", other).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": other, "Item": {"pk": {"S": "k1"}, "c": {"S": "z"}}}),
    )
    .await;

    let expected = "Can not use both expression and non-expression parameters in the same \
                    request: Non-expression parameters: {AttributesToGet} Expression parameters: \
                    {ProjectionExpression}";
    // The rows differ in WHICH entry states which generation, and in whether one entry
    // states both: every arrangement names the same two members.
    for (case, request) in [
        (
            "the legacy member in the first entry and the expression member in the second",
            json!({
                "RequestItems": {
                    TABLE: {"Keys": [{"pk": {"S": "k1"}, "sk": {"S": "s1"}}],
                            "AttributesToGet": ["a"]},
                    other: {"Keys": [{"pk": {"S": "k1"}}], "ProjectionExpression": "c"},
                },
            }),
        ),
        (
            "the expression member in the first entry and the legacy member in the second",
            json!({
                "RequestItems": {
                    TABLE: {"Keys": [{"pk": {"S": "k1"}, "sk": {"S": "s1"}}],
                            "ProjectionExpression": "a"},
                    other: {"Keys": [{"pk": {"S": "k1"}}], "AttributesToGet": ["c"]},
                },
            }),
        ),
        (
            "one entry stating both generations",
            json!({
                "RequestItems": {
                    TABLE: {"Keys": [{"pk": {"S": "k1"}, "sk": {"S": "s1"}}],
                            "AttributesToGet": ["a"], "ProjectionExpression": "a"},
                },
            }),
        ),
    ] {
        let (status, answered) = call(&router, "BatchGetItem", request).await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.to_string()),
            "case {case:?}: expected {expected:?}, got {status} with {answered}"
        );
    }

    // ONE generation across both entries is served, so the rows above are the mixing
    // being refused rather than either member being rejected.
    for (case, request) in [
        (
            "both entries stating the legacy member",
            json!({
                "RequestItems": {
                    TABLE: {"Keys": [{"pk": {"S": "k1"}, "sk": {"S": "s1"}}],
                            "AttributesToGet": ["a"]},
                    other: {"Keys": [{"pk": {"S": "k1"}}], "AttributesToGet": ["c"]},
                },
            }),
        ),
        (
            "both entries stating the expression member",
            json!({
                "RequestItems": {
                    TABLE: {"Keys": [{"pk": {"S": "k1"}, "sk": {"S": "s1"}}],
                            "ProjectionExpression": "a"},
                    other: {"Keys": [{"pk": {"S": "k1"}}], "ProjectionExpression": "c"},
                },
            }),
        ),
    ] {
        let served = ok(&router, "BatchGetItem", request).await;
        assert_eq!(
            (
                served["Responses"][TABLE][0]["a"].clone(),
                served["Responses"][other][0]["c"].clone()
            ),
            (json!({"S": "x"}), json!({"S": "z"})),
            "case {case:?}: expected each entry's projection served, got {served}"
        );
    }
}

#[tokio::test]
async fn the_mixed_parameter_message_names_each_side_in_declared_order() {
    let router = seeded().await;

    // The order each side is listed in is the order the OPERATION'S INPUT SHAPE declares
    // its members (`smithy_query(query="operation", operation="Scan")` /
    // `…"Query"`), never the order the request sent them in. The rows vary the reader,
    // which pair collides, how many members each side carries, and the ORDER THE REQUEST
    // states them in, which the message is indifferent to.
    // The last field is whether the refusal carries the `1 validation error detected: `
    // envelope: it follows the MEMBERS the two sides name, so a READ stating its
    // projection, its filter or its key condition in both generations is refused with the
    // sentence alone, and a WRITE stating its condition in both is refused inside the
    // envelope.
    let rows: Vec<(&str, &str, Value, &str, &str, bool)> = vec![
        (
            "a Scan carrying every parameter of both generations",
            "Scan",
            json!({
                "TableName": TABLE,
                "FilterExpression": "a = :v",
                "ProjectionExpression": "b",
                "ExpressionAttributeValues": {":v": {"S": "x"}},
                "ConditionalOperator": "AND",
                "ScanFilter": filter_entries(&["a"]),
                "AttributesToGet": ["a"],
            }),
            "AttributesToGet, ScanFilter, ConditionalOperator",
            "ProjectionExpression, FilterExpression",
            false,
        ),
        (
            "a Scan carrying one legacy member and both expression members",
            "Scan",
            json!({
                "TableName": TABLE,
                "FilterExpression": "a = :v",
                "ProjectionExpression": "b",
                "ExpressionAttributeValues": {":v": {"S": "x"}},
                "ScanFilter": filter_entries(&["a"]),
            }),
            "ScanFilter",
            "ProjectionExpression, FilterExpression",
            false,
        ),
        // This row pins the RULE applied uniformly — both sides in the order
        // `smithy_query(query="operation", operation="Query")` declares them. For a
        // request carrying ALL of both generations AWS is observed to state a different
        // order inside the braces, which no rule reachable from the model or the docs
        // explains; the resolved issue report records that divergence rather than fitting
        // this row to it.
        (
            "a Query carrying every parameter of both generations",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditionExpression": "pk = :k",
                "FilterExpression": "a = :v",
                "ProjectionExpression": "b",
                "ExpressionAttributeValues": {":k": {"S": "k1"}, ":v": {"S": "x"}},
                "ConditionalOperator": "AND",
                "QueryFilter": filter_entries(&["a"]),
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "AttributesToGet": ["a"],
            }),
            "AttributesToGet, KeyConditions, QueryFilter, ConditionalOperator",
            "ProjectionExpression, FilterExpression, KeyConditionExpression",
            false,
        ),
        (
            "a Query carrying every parameter but the legacy key condition",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditionExpression": "pk = :k",
                "FilterExpression": "a = :v",
                "ProjectionExpression": "b",
                "ExpressionAttributeValues": {":k": {"S": "k1"}, ":v": {"S": "x"}},
                "ConditionalOperator": "AND",
                "QueryFilter": filter_entries(&["a"]),
                "AttributesToGet": ["a"],
            }),
            "AttributesToGet, QueryFilter, ConditionalOperator",
            "ProjectionExpression, FilterExpression, KeyConditionExpression",
            false,
        ),
        (
            "a Query stating only its key condition both ways",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditionExpression": "pk = :k",
                "ExpressionAttributeValues": {":k": {"S": "k1"}},
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
            }),
            "KeyConditions",
            "KeyConditionExpression",
            false,
        ),
        (
            "a Query carrying ConditionalOperator beside a key condition expression",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditionExpression": "pk = :k",
                "ExpressionAttributeValues": {":k": {"S": "k1"}},
                "ConditionalOperator": "AND",
            }),
            "ConditionalOperator",
            "KeyConditionExpression",
            false,
        ),
        (
            "a Query carrying the key members of both generations and a filter expression",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditionExpression": "pk = :k",
                "FilterExpression": "a = :v",
                "ExpressionAttributeValues": {":k": {"S": "k1"}, ":v": {"S": "x"}},
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
            }),
            "KeyConditions",
            "FilterExpression, KeyConditionExpression",
            false,
        ),
        (
            "a Scan carrying ConditionalOperator beside a projection expression",
            "Scan",
            json!({
                "TableName": TABLE,
                "ConditionalOperator": "AND",
                "ProjectionExpression": "a",
            }),
            "ConditionalOperator",
            "ProjectionExpression",
            false,
        ),
        (
            "a write stating its condition both ways",
            "PutItem",
            json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "k7"}, "sk": {"S": "s7"}},
                "ConditionExpression": "attribute_not_exists(a)",
                "ConditionalOperator": "AND",
                "Expected": expected_entries(&["a", "b"]),
            }),
            "Expected, ConditionalOperator",
            "ConditionExpression",
            true,
        ),
        // A legacy member beside an expression member that states something ELSE is
        // refused just the same: what the service holds against a request is the two
        // GENERATIONS, not a legacy member beside the one that replaced it, so a
        // projection in one generation and a filter in the other collide too.
        (
            "a Query stating its key condition the legacy way and its projection the new way",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "ProjectionExpression": "a",
            }),
            "KeyConditions",
            "ProjectionExpression",
            false,
        ),
        (
            "a Scan stating its filter the legacy way and its projection the new way",
            "Scan",
            json!({
                "TableName": TABLE,
                "ScanFilter": filter_entries(&["a"]),
                "ProjectionExpression": "a",
            }),
            "ScanFilter",
            "ProjectionExpression",
            false,
        ),
        (
            "a Query stating its filter the legacy way and its key condition the new way",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditionExpression": "pk = :k",
                "ExpressionAttributeValues": {":k": {"S": "k1"}},
                "QueryFilter": filter_entries(&["a", "b"]),
            }),
            "QueryFilter",
            "KeyConditionExpression",
            false,
        ),
        (
            "a write stating its condition the legacy way and its update the new way",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "UpdateExpression": "SET c = :z",
                "ExpressionAttributeValues": {":z": {"S": "z"}},
                "Expected": expected_entries(&["nothere", "alsonot"]),
            }),
            "Expected",
            "UpdateExpression",
            true,
        ),
    ];

    for (case, operation, request, legacy, expression, enveloped) in rows {
        let envelope = if enveloped {
            "1 validation error detected: "
        } else {
            ""
        };
        let expected = format!(
            "{envelope}Can not use both expression and non-expression parameters in the same \
             request: Non-expression parameters: {{{legacy}}} Expression parameters: \
             {{{expression}}}"
        );
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected {expected:?}, got {status} with {answered}"
        );
    }

    // A request that stays inside ONE generation is served, whichever generation it
    // picked: the rule is about carrying members of BOTH, not about carrying a legacy
    // member at all.
    for (case, operation, request) in [
        (
            "a Query stating both its key condition and its projection the legacy way",
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditions": {
                    "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
                },
                "AttributesToGet": ["a"],
            }),
        ),
        (
            "a Scan stating both its filter and its projection the legacy way",
            "Scan",
            json!({
                "TableName": TABLE,
                "ScanFilter": filter_entries(&["a", "b"]),
                "ConditionalOperator": "AND",
                "AttributesToGet": ["a"],
            }),
        ),
        (
            "a Scan stating both its filter and its projection the expression way",
            "Scan",
            json!({
                "TableName": TABLE,
                "FilterExpression": "attribute_exists(a)",
                "ProjectionExpression": "a",
            }),
        ),
        (
            "a write stating both its condition and its update the expression way",
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": {"pk": {"S": "k1"}, "sk": {"S": "s1"}},
                "UpdateExpression": "SET c = :z",
                "ConditionExpression": "attribute_exists(pk)",
                "ExpressionAttributeValues": {":z": {"S": "z"}},
            }),
        ),
    ] {
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a request inside one generation is served, got {status} with \
             {answered}"
        );
    }
}
