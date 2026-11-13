// SPDX-License-Identifier: CC0-1.0

//! Which members a mixed-generation refusal names, and in what ORDER.
//!
//! THE RULE: the expression language SUPERSEDES the legacy (non-expression) members that
//! narrow a read — its projection, its filter, the operator that combines the filter map's
//! entries — so a reader carrying one of those beside ANY expression member is refused 400
//! `ValidationException` naming every superseded member it carried, by the ASK each states:
//! a request's projection, its filter, its key condition, walked in the order the operation
//! declares the expression members those asks are stated through, members of one ask keeping
//! the order the operation declares them in. The expression side names every expression
//! member the request carried, in that same declared order. The order the REQUEST sent its
//! members in never reaches the message.
//!
//! The one legacy member that is not narrowing is the one a `Query` states its KEY CONDITION
//! through, which the reader cannot run without: it is resolved from whichever generation
//! carries it and is held only against the member that replaced it, so a `Query` keying the
//! legacy way and filtering or projecting the expression way is SERVED.
//!
//! The members and their declared order are the ones
//! `smithy_query(query="operation", operation="Query")` and `…"Scan"` give:
//! `AttributesToGet`, `KeyConditions`, `QueryFilter`, `ConditionalOperator` against
//! `ProjectionExpression`, `FilterExpression`, `KeyConditionExpression` for a `Query`, and
//! `AttributesToGet`, `ScanFilter`, `ConditionalOperator` against `ProjectionExpression`,
//! `FilterExpression` for a `Scan`. The ask each legacy member states is the one the member
//! that replaced it states, and a `ConditionalOperator` states the filter's ask, because it
//! combines the entries of the legacy filter map it is sent with.
//!
//! Each table walks EVERY non-empty subset of its reader's legacy members against EVERY
//! non-empty subset of its expression members, so the rule is asserted over which members
//! are carried, how many, and which asks the request states — not over one recorded request.
//! Every other subset inserts its legacy members into the request in the reverse order, so a
//! message that followed the request's own order would fail on half the rows. A subset whose
//! only legacy member is the key condition is held to the other half of the rule: it is
//! served. The controls at the end pin that a request staying inside ONE generation is
//! served too.
//!
//! Everything is driven over the wire, so nothing is asserted that a client could not
//! observe.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table every row below reads: a composite key, so a `Query` can state a key
/// condition, plus the non-key attributes the projections and filters name.
const TABLE: &str = "MixedParameterOrder";

/// `Query`'s non-expression members, in the order `QueryInput` declares them, each with the
/// ask it states — the position of the member that replaced it in [`QUERY_EXPRESSION`].
const QUERY_LEGACY: [(&str, usize); 4] = [
    ("AttributesToGet", 0),
    ("KeyConditions", 2),
    ("QueryFilter", 1),
    ("ConditionalOperator", 1),
];

/// `Query`'s expression members, in the order `QueryInput` declares them.
const QUERY_EXPRESSION: [&str; 3] = [
    "ProjectionExpression",
    "FilterExpression",
    "KeyConditionExpression",
];

/// `Scan`'s non-expression members, in the order `ScanInput` declares them, each with the
/// ask it states — the position of the member that replaced it in [`SCAN_EXPRESSION`].
const SCAN_LEGACY: [(&str, usize); 3] = [
    ("AttributesToGet", 0),
    ("ScanFilter", 1),
    ("ConditionalOperator", 1),
];

/// `Scan`'s expression members, in the order `ScanInput` declares them.
const SCAN_EXPRESSION: [&str; 2] = ["ProjectionExpression", "FilterExpression"];

/// The sentence a mixed request earns, with both sides filled in. A read states it on its
/// own, outside the `<N> validation error(s) detected: ` envelope a write's carries.
fn mixed(legacy: &[&str], expression: &[&str]) -> String {
    format!(
        "Can not use both expression and non-expression parameters in the same request: \
         Non-expression parameters: {{{}}} Expression parameters: {{{}}}",
        legacy.join(", "),
        expression.join(", ")
    )
}

/// What a request sends for one legacy member — a value the member's own shape admits, so
/// the refusal is about the generations and not about any member's content.
fn legacy_value(member: &str) -> Value {
    match member {
        "AttributesToGet" => json!(["a"]),
        "ConditionalOperator" => json!("AND"),
        "KeyConditions" => json!({
            "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "k1"}]},
        }),
        // The two filter maps are the same shape, carrying two entries so that a request
        // sending `ConditionalOperator` beside one has entries for it to combine.
        _ => json!({
            "a": {"ComparisonOperator": "NOT_NULL"},
            "b": {"ComparisonOperator": "NOT_NULL"},
        }),
    }
}

/// What a request sends for one expression member, over the two attributes the table holds.
fn expression_value(member: &str) -> Value {
    match member {
        "ProjectionExpression" => json!("b"),
        "FilterExpression" => json!("a = :v"),
        _ => json!("pk = :k"),
    }
}

/// The request one row sends: the table, the expression members `expression` names, the
/// values those members bind, and the legacy members `legacy` names — inserted in the order
/// given, which the message is indifferent to.
fn request(expression: &[&str], legacy: &[&str]) -> Value {
    let mut body = Map::new();
    body.insert("TableName".to_string(), json!(TABLE));
    let mut values = Map::new();
    for member in expression {
        body.insert((*member).to_string(), expression_value(member));
        if *member == "FilterExpression" {
            values.insert(":v".to_string(), json!({"S": "x"}));
        }
        if *member == "KeyConditionExpression" {
            values.insert(":k".to_string(), json!({"S": "k1"}));
        }
    }
    if !values.is_empty() {
        body.insert(
            "ExpressionAttributeValues".to_string(),
            Value::Object(values),
        );
    }
    for member in legacy {
        body.insert((*member).to_string(), legacy_value(member));
    }
    Value::Object(body)
}

/// The non-expression side as the refusal names it: every superseded legacy member the
/// request carried, by the ask each states, keeping the operation's declared order inside
/// one ask.
///
/// `stated` holds the asks the request stated the expression way — the positions of the
/// expression members it carried, which is non-empty wherever this is called. Every legacy
/// member the expression language supersedes is named; the KEY CONDITION is named only when
/// the request also carried the member that replaced it. EMPTY means nothing was
/// superseded, which is a request this rule does not refuse at all.
fn named_legacy<'m>(legacy: &[(&'m str, usize)], stated: &[usize]) -> Vec<&'m str> {
    let mut superseded: Vec<(usize, &str)> = legacy
        .iter()
        .filter(|(member, ask)| *member != KEY_CONDITION || stated.contains(ask))
        .map(|(member, ask)| (*ask, *member))
        .collect();
    superseded.sort_by_key(|(ask, _)| *ask);
    superseded.into_iter().map(|(_, member)| member).collect()
}

/// The legacy member a `Query` states its key condition through, which the reader cannot run
/// without and so takes from either generation.
const KEY_CONDITION: &str = "KeyConditions";

/// Every non-empty subset of `members`, as a vector of its entries.
fn subsets<T: Copy>(members: &[T]) -> Vec<Vec<T>> {
    (1u32..(1 << members.len()))
        .map(|mask| {
            members
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) != 0)
                .map(|(_, member)| *member)
                .collect()
        })
        .collect()
}

/// An emulator holding one active table with one item in it, so a served control is served
/// against real state.
async fn seeded() -> MultiServiceRouter {
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
    describe_when_active(&router, "the mixed-parameter table", TABLE).await;
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

/// Drive every subset of one reader's two generations against the other: a request carrying
/// a superseded legacy member is held to the rule's wording and order, and one whose only
/// legacy member is the key condition is served.
///
/// A `Query` states its key condition in one generation or the other, so a served row of
/// that reader is only driven where the subsets carry one; a subset carrying neither is
/// skipped rather than asserted on, because what such a request earns is the key-condition
/// requirement and not this rule.
async fn every_mix(
    router: &MultiServiceRouter,
    operation: &str,
    legacy_members: &[(&'static str, usize)],
    expression_members: &[&'static str],
    keying: Option<(&str, &str)>,
) {
    let names: Vec<&str> = legacy_members.iter().map(|(member, _)| *member).collect();
    for (round, carried_legacy) in subsets(&names).into_iter().enumerate() {
        // Every other row sends its legacy members in the reverse order, so a message that
        // followed the request would disagree with the rule on half the rows.
        let mut sent = carried_legacy.clone();
        if round % 2 == 0 {
            sent.reverse();
        }
        for carried_expression in subsets(expression_members) {
            let stated: Vec<usize> = expression_members
                .iter()
                .enumerate()
                .filter(|(_, member)| carried_expression.contains(member))
                .map(|(index, _)| index)
                .collect();
            let carried: Vec<(&str, usize)> = legacy_members
                .iter()
                .filter(|(member, _)| carried_legacy.contains(member))
                .copied()
                .collect();
            let superseded = named_legacy(&carried, &stated);
            let case = format!("a {operation} carrying {sent:?} beside {carried_expression:?}");
            let (status, answered) =
                call(router, operation, request(&carried_expression, &sent)).await;
            if superseded.is_empty() {
                // Nothing superseded: the request is served, as long as it states the one
                // member this reader cannot do without.
                if keying.is_some_and(|(legacy, expression)| {
                    !sent.contains(&legacy) && !carried_expression.contains(&expression)
                }) {
                    continue;
                }
                assert_eq!(
                    status, 200,
                    "case {case}: nothing is superseded, so the request is served, got \
                     {status} with {answered}"
                );
                continue;
            }
            let expected = mixed(&superseded, &carried_expression);
            assert_eq!(
                (status, error_code(&answered).as_str(), message(&answered)),
                (400, "ValidationException", expected.clone()),
                "case {case}: expected {expected:?}, got {status} with {answered}"
            );
        }
    }
}

#[tokio::test]
async fn a_query_names_each_side_by_the_ask_its_members_state() {
    let router = seeded().await;

    every_mix(
        &router,
        "Query",
        &QUERY_LEGACY,
        &QUERY_EXPRESSION,
        Some((KEY_CONDITION, "KeyConditionExpression")),
    )
    .await;

    // The control: a Query that stays inside ONE generation is served, so the refusals
    // above are about a superseded member and not about the legacy members themselves.
    // Every legacy member together: the key condition selects the stored item, the filter
    // keeps it, and the legacy projection narrows it to the one attribute it names.
    let names: Vec<&str> = QUERY_LEGACY.iter().map(|(member, _)| *member).collect();
    let served = ok(&router, "Query", request(&[], &names)).await;
    assert_eq!(
        (served["Count"].clone(), served["Items"].clone()),
        (json!(1), json!([{"a": {"S": "x"}}])),
        "case a Query inside the legacy generation: it must be served, got {served}"
    );
}

#[tokio::test]
async fn a_scan_names_each_side_by_the_ask_its_members_state() {
    let router = seeded().await;

    every_mix(&router, "Scan", &SCAN_LEGACY, &SCAN_EXPRESSION, None).await;

    // The same control on this reader: every legacy member together, with no expression
    // member beside them, is served.
    let names: Vec<&str> = SCAN_LEGACY.iter().map(|(member, _)| *member).collect();
    let served = ok(&router, "Scan", request(&[], &names)).await;
    assert_eq!(
        (served["Count"].clone(), served["Items"].clone()),
        (json!(1), json!([{"a": {"S": "x"}}])),
        "case a Scan inside the legacy generation: it must be served, got {served}"
    );
}
