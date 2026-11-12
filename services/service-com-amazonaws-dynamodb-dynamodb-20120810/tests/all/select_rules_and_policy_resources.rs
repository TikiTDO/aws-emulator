// SPDX-License-Identifier: CC0-1.0

//! The rules a READ's `Select`/projection pair, a legacy filter and a resource-based
//! policy's own resource are held to, driven end to end.
//!
//! The rules pinned here, each over a SET of inputs rather than one example:
//!
//! 1. `Select` and a projection are held to each other, and the refusal is one clause
//!    inside the validation envelope: a projection beside any `Select` but
//!    `SPECIFIC_ATTRIBUTES` cannot be served ("If you use the ProjectionExpression
//!    parameter, then the value for Select can only be SPECIFIC_ATTRIBUTES" —
//!    `Query$Select`), `SPECIFIC_ATTRIBUTES` with no projection has nothing to return,
//!    and `ALL_PROJECTED_ATTRIBUTES` is "allowed only when querying an index". The
//!    clause names the member the request carried its projection in and the verb of the
//!    operation that was called;
//! 2. a LEGACY `QueryFilter` states the same filter a `FilterExpression` does — "This is
//!    a legacy parameter. Use FilterExpression instead" (`Query$QueryFilter`) — so it is
//!    SERVED, and a comparison given the wrong number of `AttributeValueList` entries is
//!    the client error it is rather than an unimplemented feature;
//! 3. a substitution table is "substitution tokens for attribute names in an expression"
//!    (`ExpressionAttributeNames`), so a request that carries one with no expression at
//!    all is told the member needs one, wherever such a member is accepted;
//! 4. an operand a key CONDITION compares a key attribute with is held to the type the
//!    schema declares for it, with the sentence a condition parameter earns;
//! 5. a resource-based policy is attached to "a table or stream" (the
//!    `PutResourcePolicy` doc): a stream holds a policy of its own, independent of its
//!    table's, every statement names actions of the resource it is attached to, an Allow
//!    may not name a public principal, and the document is stored as the request stated
//!    it — nothing is added to it, the `Version` it is read under included.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok, REGION};

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The envelope a violation of a rule over a request's own members is stated inside.
const ENVELOPE: &str = "1 validation error detected: ";

/// Wait out the span a policy write takes to be applied, after which the eventually
/// consistent reader reports it — the span the store itself owes it
/// (`crate::state::STATUS_TRANSITION`), plus room for the round trips.
async fn wait_for_policy_application() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(200),
    )
    .await;
}

/// One table with a string partition key and a numeric sort key, plus a global secondary
/// index over a numeric attribute, created and waited to `ACTIVE`.
async fn seeded(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                          {"AttributeName": "sk", "KeyType": "RANGE"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "N"},
                {"AttributeName": "rank", "AttributeType": "N"},
            ],
            "GlobalSecondaryIndexes": [{
                "IndexName": "ByRank",
                "KeySchema": [{"AttributeName": "rank", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    crate::emulator::describe_when_active(router, "seeded", name).await;
    for (sort, rank, label) in [(1, 10, "first"), (2, 20, "second"), (3, 30, "third")] {
        ok(
            router,
            "PutItem",
            json!({"TableName": name,
                   "Item": {"pk": {"S": "p1"}, "sk": {"N": sort.to_string()},
                            "rank": {"N": rank.to_string()},
                            "label": {"S": label}}}),
        )
        .await;
    }
}

/// The `pk = :p` key condition every read below selects the seeded partition with.
fn key_condition() -> Value {
    json!({"KeyConditionExpression": "pk = :p",
           "ExpressionAttributeValues": {":p": {"S": "p1"}}})
}

/// One request body: the base members plus the row's own.
fn body(base: &Value, extra: &Value) -> Value {
    let mut merged = base.as_object().cloned().unwrap_or_default();
    for (name, value) in extra.as_object().into_iter().flatten() {
        merged.insert(name.clone(), value.clone());
    }
    Value::Object(merged)
}

#[tokio::test]
async fn select_and_a_projection_are_held_to_each_other_in_the_words_aws_uses() {
    let router = emulator();
    let table = "SelectRules";
    seeded(&router, table).await;

    // Rows differ along what the rule generalises over: the operation the rule is applied
    // for (a query and a scan, whose refusal names the verb of the call), the `Select`
    // value (every value the model's `Select` enum declares bar `SPECIFIC_ATTRIBUTES`),
    // the member the projection arrived in (`ProjectionExpression` and the legacy
    // `AttributesToGet`), and whether an index was named.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a query of every attribute beside a projection",
            "Query",
            json!({"Select": "ALL_ATTRIBUTES", "ProjectionExpression": "label"}),
            "Cannot specify the ProjectionExpression when choosing to get ALL_ATTRIBUTES"
                .to_string(),
        ),
        (
            "a query counting beside a projection",
            "Query",
            json!({"Select": "COUNT", "ProjectionExpression": "label"}),
            "Cannot specify the ProjectionExpression when choosing to get only the Count"
                .to_string(),
        ),
        (
            "a query counting beside the legacy projection member",
            "Query",
            json!({"Select": "COUNT", "AttributesToGet": ["label"],
                   "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                            "AttributeValueList": [{"S": "p1"}]}}}),
            "Cannot specify the AttributesToGet when choosing to get only the Count".to_string(),
        ),
        (
            "a query of an index's projection beside a projection",
            "Query",
            json!({"IndexName": "ByRank", "Select": "ALL_PROJECTED_ATTRIBUTES",
                   "ProjectionExpression": "label"}),
            "Cannot specify the ProjectionExpression when choosing to get \
             ALL_PROJECTED_ATTRIBUTES"
                .to_string(),
        ),
        (
            "a query of specific attributes with nothing listed",
            "Query",
            json!({"Select": "SPECIFIC_ATTRIBUTES"}),
            "Must specify the AttributesToGet or ProjectionExpression when choosing to get \
             SPECIFIC_ATTRIBUTES"
                .to_string(),
        ),
        (
            "a query of an index's projection without an index",
            "Query",
            json!({"Select": "ALL_PROJECTED_ATTRIBUTES"}),
            "ALL_PROJECTED_ATTRIBUTES can be used only when Querying using an IndexName"
                .to_string(),
        ),
        (
            "a scan of every attribute beside a projection",
            "Scan",
            json!({"Select": "ALL_ATTRIBUTES", "ProjectionExpression": "label"}),
            "Cannot specify the ProjectionExpression when choosing to get ALL_ATTRIBUTES"
                .to_string(),
        ),
        (
            "a scan of specific attributes with nothing listed",
            "Scan",
            json!({"Select": "SPECIFIC_ATTRIBUTES"}),
            "Must specify the AttributesToGet or ProjectionExpression when choosing to get \
             SPECIFIC_ATTRIBUTES"
                .to_string(),
        ),
        (
            "a scan of an index's projection without an index",
            "Scan",
            json!({"Select": "ALL_PROJECTED_ATTRIBUTES"}),
            // The sentence is the rule's own, so a Scan is refused in the SAME words a
            // Query is: nothing in it is taken from the operation that was called.
            "ALL_PROJECTED_ATTRIBUTES can be used only when Querying using an IndexName"
                .to_string(),
        ),
    ];

    for (case, operation, extra, clause) in &rows {
        // A request states its intent through the expression members or through the
        // legacy ones they replaced, never through both, so a row that projects through
        // the legacy member states its key condition the legacy way too.
        let base = match (*operation, extra.get("KeyConditions")) {
            ("Query", None) => body(&json!({"TableName": table}), &key_condition()),
            _ => json!({"TableName": table}),
        };
        let (status, answered) = call(&router, operation, body(&base, extra)).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
        );
        let said = message(&answered);
        assert_eq!(
            said,
            format!("{ENVELOPE}{clause}"),
            "case {case:?}: expected {ENVELOPE}{clause}, got {said:?}"
        );
    }

    // The control: the one pairing the rule allows is SERVED, through either projection
    // member, and reports exactly what it listed.
    for (case, extra) in [
        (
            "the modern member",
            json!({"Select": "SPECIFIC_ATTRIBUTES", "ProjectionExpression": "label"}),
        ),
        (
            "the legacy member",
            json!({"Select": "SPECIFIC_ATTRIBUTES", "AttributesToGet": ["label"]}),
        ),
    ] {
        // A request states its intent through the expression members or through the legacy
        // ones they replaced, never through both, so the legacy row states its key
        // condition the legacy way.
        let base = match extra.get("AttributesToGet") {
            Some(_) => json!({"TableName": table,
                              "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                                       "AttributeValueList": [{"S": "p1"}]}}}),
            None => body(&json!({"TableName": table}), &key_condition()),
        };
        let served = ok(&router, "Query", body(&base, &extra)).await;
        let items = served["Items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            3,
            "case {case:?}: every item of the partition is still reported, got {served}"
        );
        for item in &items {
            let names: Vec<&String> = item.as_object().into_iter().flat_map(|map| map.keys()).collect();
            assert_eq!(
                names,
                vec![&"label".to_string()],
                "case {case:?}: only the listed attribute is reported, got {item}"
            );
        }
    }
}

#[tokio::test]
async fn a_legacy_query_filter_is_served_and_its_argument_rules_are_client_errors() {
    let router = emulator();
    let table = "LegacyQueryFilter";
    seeded(&router, table).await;

    // Rows differ along the comparator and the operands it takes: the filter selects the
    // items of the partition that satisfy it, exactly as the expression form does.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "an equality on a non-key attribute",
            json!({"label": {"ComparisonOperator": "EQ",
                             "AttributeValueList": [{"S": "second"}]}}),
            vec!["second"],
        ),
        (
            "a range on a projected number",
            json!({"rank": {"ComparisonOperator": "GE",
                            "AttributeValueList": [{"N": "20"}]}}),
            vec!["second", "third"],
        ),
        (
            "a BETWEEN over two operands",
            json!({"rank": {"ComparisonOperator": "BETWEEN",
                            "AttributeValueList": [{"N": "5"}, {"N": "15"}]}}),
            vec!["first"],
        ),
        (
            "an existence test taking no operand",
            json!({"missing": {"ComparisonOperator": "NULL"}}),
            vec!["first", "second", "third"],
        ),
    ];

    for (case, filter, expected) in &rows {
        let served = ok(
            &router,
            "Query",
            json!({"TableName": table,
                   "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                            "AttributeValueList": [{"S": "p1"}]}},
                   "QueryFilter": filter}),
        )
        .await;
        let labels: Vec<String> = served["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| item["label"]["S"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            labels,
            expected.iter().map(|label| (*label).to_string()).collect::<Vec<String>>(),
            "case {case:?}: expected the filter to select {expected:?}, got {served}"
        );
        assert_eq!(
            served["ScannedCount"],
            json!(3),
            "case {case:?}: every item of the partition was evaluated, got {served}"
        );
    }

    // A comparator given the wrong number of operands is a bad REQUEST — never an
    // unimplemented feature — whichever of the two readers carries the legacy map.
    for (case, operation, filter) in [
        (
            "an equality with two operands",
            "Query",
            json!({"label": {"ComparisonOperator": "EQ",
                             "AttributeValueList": [{"S": "a"}, {"S": "b"}]}}),
        ),
        (
            "an existence test with an operand",
            "Query",
            json!({"label": {"ComparisonOperator": "NOT_NULL",
                             "AttributeValueList": [{"S": "a"}]}}),
        ),
        (
            "a BETWEEN with one operand",
            "Query",
            json!({"rank": {"ComparisonOperator": "BETWEEN",
                            "AttributeValueList": [{"N": "1"}]}}),
        ),
        (
            "the same defect on a scan",
            "Scan",
            json!({"label": {"ComparisonOperator": "EQ",
                             "AttributeValueList": [{"S": "a"}, {"S": "b"}]}}),
        ),
    ] {
        let request = match operation {
            "Query" => json!({"TableName": table,
                              "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                                       "AttributeValueList": [{"S": "p1"}]}},
                              "QueryFilter": filter}),
            _ => json!({"TableName": table, "ScanFilter": filter}),
        };
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): an illegal argument list is the client error, got \
             {status} with {answered}"
        );
    }
}

#[tokio::test]
async fn a_substitution_table_with_no_expression_names_the_member_that_needs_one() {
    let router = emulator();
    let table = "SubstitutionWithoutExpression";
    seeded(&router, table).await;

    // Rows differ along the operation that accepts the member (an item read, a batch
    // read, a transactional read, a query and a scan) and along which table was sent.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an item read",
            json!({"TableName": table, "Key": {"pk": {"S": "p1"}, "sk": {"N": "1"}},
                   "ExpressionAttributeNames": {"#l": "label"}}),
            "ExpressionAttributeNames",
        ),
        (
            "a query stating its key condition the legacy way",
            json!({"TableName": table,
                   "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                            "AttributeValueList": [{"S": "p1"}]}},
                   "ExpressionAttributeNames": {"#l": "label"}}),
            "ExpressionAttributeNames",
        ),
        (
            "a scan",
            json!({"TableName": table, "ExpressionAttributeNames": {"#l": "label"}}),
            "ExpressionAttributeNames",
        ),
        (
            "a batch read",
            json!({"RequestItems": {table: {
                "Keys": [{"pk": {"S": "p1"}, "sk": {"N": "1"}}],
                "ExpressionAttributeNames": {"#l": "label"}}}}),
            "ExpressionAttributeNames",
        ),
        (
            "a transactional read",
            json!({"TransactItems": [{"Get": {
                "TableName": table,
                "Key": {"pk": {"S": "p1"}, "sk": {"N": "1"}},
                "ExpressionAttributeNames": {"#l": "label"}}}]}),
            "ExpressionAttributeNames",
        ),
    ];
    let operations = ["GetItem", "Query", "Scan", "BatchGetItem", "TransactGetItems"];

    for (operation, (case, request, member)) in operations.iter().zip(rows) {
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("{member} can only be specified when using expressions"),
            "case {case:?}: the refusal names the member, got {}",
            message(&answered)
        );
    }

    // The control: the same member beside an expression that USES it is served.
    let served = ok(
        &router,
        "GetItem",
        json!({"TableName": table, "Key": {"pk": {"S": "p1"}, "sk": {"N": "1"}},
               "ProjectionExpression": "#l",
               "ExpressionAttributeNames": {"#l": "label"}}),
    )
    .await;
    assert_eq!(
        served["Item"]["label"],
        json!({"S": "first"}),
        "case the member beside an expression that uses it: expected the projected item, got \
         {served}"
    );
}

#[tokio::test]
async fn a_key_condition_operand_of_another_type_is_the_condition_parameter_refusal() {
    let router = emulator();
    let table = "ConditionParameterTypes";
    seeded(&router, table).await;

    // Rows differ along the schema the condition reads (the table's own key and an
    // index's), which key attribute the operand compares, the declared type it misses
    // (`S` and `N`) and the member the condition arrived in.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a number for the string partition key",
            json!({"TableName": table, "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"N": "1"}}}),
        ),
        (
            "a string for the numeric sort key",
            json!({"TableName": table, "KeyConditionExpression": "pk = :p AND sk = :s",
                   "ExpressionAttributeValues": {":p": {"S": "p1"}, ":s": {"S": "1"}}}),
        ),
        (
            "a string for the numeric index key",
            json!({"TableName": table, "IndexName": "ByRank",
                   "KeyConditionExpression": "#r = :r",
                   "ExpressionAttributeNames": {"#r": "rank"},
                   "ExpressionAttributeValues": {":r": {"S": "10"}}}),
        ),
        (
            "the same defect through the legacy member",
            json!({"TableName": table,
                   "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                            "AttributeValueList": [{"N": "1"}]}}}),
        ),
    ];

    for (case, request) in &rows {
        let (status, answered) = call(&router, "Query", request.clone()).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
        );
        assert_eq!(
            message(&answered),
            "One or more parameter values were invalid: Condition parameter type does not match \
             schema type",
            "case {case:?}: the refusal states the rule the parameter broke, got {}",
            message(&answered)
        );
    }

    // The control: an operand OF the declared type is served, and a key the request
    // ADDRESSES an item by keeps its own sentence, which names the attribute.
    let served = ok(
        &router,
        "Query",
        json!({"TableName": table, "KeyConditionExpression": "pk = :p AND sk = :s",
               "ExpressionAttributeValues": {":p": {"S": "p1"}, ":s": {"N": "2"}}}),
    )
    .await;
    assert_eq!(
        served["Count"],
        json!(1),
        "case an operand of the declared type: expected the one item it selects, got {served}"
    );
    let (status, answered) = call(
        &router,
        "GetItem",
        json!({"TableName": table, "Key": {"pk": {"N": "1"}, "sk": {"N": "1"}}}),
    )
    .await;
    assert_eq!(
        (status, message(&answered).as_str()),
        (400, "The provided key element does not match the schema"),
        "case a key the request addresses an item by: expected the key-map sentence, got \
         {answered}"
    );
}

#[tokio::test]
async fn a_stream_holds_a_policy_of_its_own_and_every_statement_names_the_resources_actions() {
    let router = emulator();
    let table = "StreamPolicies";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "NEW_IMAGE"},
        }),
    )
    .await;
    let described = crate::emulator::describe_when_active(&router, "stream policies", table).await;
    let stream_arn = described["LatestStreamArn"]
        .as_str()
        .expect("case stream policies: a stream-enabled table reports its stream ARN")
        .to_string();
    let table_arn = described["TableArn"]
        .as_str()
        .expect("case stream policies: the description reports the table ARN")
        .to_string();

    // One policy per resource, each naming that resource's own actions: the two are held
    // independently, so each reads back as the document it was attached with.
    let table_policy = json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Deny", "Principal": {"AWS": "*"},
                       "Action": "dynamodb:DeleteItem", "Resource": &table_arn}],
    })
    .to_string();
    let stream_policy = json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Deny", "Principal": {"AWS": "*"},
                       "Action": "dynamodb:GetRecords", "Resource": &stream_arn}],
    })
    .to_string();
    for (case, arn, document) in [
        ("the table", &table_arn, &table_policy),
        ("its stream", &stream_arn, &stream_policy),
    ] {
        let attached = ok(
            &router,
            "PutResourcePolicy",
            json!({"ResourceArn": arn, "Policy": document}),
        )
        .await;
        assert!(
            attached["RevisionId"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: the attach reports a revision id, got {attached}"
        );
    }
    // Each attach is asynchronous, so the documents are read back once they are applied.
    wait_for_policy_application().await;
    for (case, arn, document) in [
        ("the table", &table_arn, &table_policy),
        ("its stream", &stream_arn, &stream_policy),
    ] {
        let read = ok(
            &router,
            "GetResourcePolicy",
            json!({"ResourceArn": arn}),
        )
        .await;
        assert_eq!(
            read["Policy"],
            json!(document),
            "case {case:?}: each resource reports its OWN document, got {read}"
        );
    }
    // Removing one leaves the other where it was: the two are not one policy.
    ok(
        &router,
        "DeleteResourcePolicy",
        json!({"ResourceArn": &stream_arn}),
    )
    .await;
    // The removal is asynchronous like the attach, so the reader is asked once it is
    // applied.
    wait_for_policy_application().await;
    let (status, answered) = call(
        &router,
        "GetResourcePolicy",
        json!({"ResourceArn": &stream_arn}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "PolicyNotFoundException"),
        "case the stream after its policy was removed: expected PolicyNotFoundException, got \
         {status} with {answered}"
    );
    let kept = ok(&router, "GetResourcePolicy", json!({"ResourceArn": &table_arn})).await;
    assert_eq!(
        kept["Policy"],
        json!(&table_policy),
        "case the table after the stream's policy was removed: its own policy stands, got {kept}"
    );

    // A statement that names an action the resource does not have, a public principal in
    // an Allow, a document that is not JSON, and a resource that is neither a table nor a
    // stream, are each refused — and none of them changes what the table carries.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a table action in a stream's policy",
            json!({"ResourceArn": &stream_arn, "Policy": json!({
                "Version": "2012-10-17",
                "Statement": [{"Effect": "Deny", "Principal": {"AWS": "*"},
                               "Action": "dynamodb:GetItem", "Resource": &stream_arn}],
            }).to_string()}),
            "One or more parameter values were invalid: Invalid policy document: Policy has an \
             action that is not supported for the resource: dynamodb:GetItem"
                .to_string(),
        ),
        (
            "an action of another service",
            json!({"ResourceArn": &table_arn, "Policy": json!({
                "Version": "2012-10-17",
                "Statement": [{"Effect": "Deny", "Principal": {"AWS": "*"},
                               "Action": "s3:GetObject", "Resource": &table_arn}],
            }).to_string()}),
            "One or more parameter values were invalid: Invalid policy document: Policy has an \
             action that is not supported for the resource: s3:GetObject"
                .to_string(),
        ),
        (
            "an Allow naming a public principal",
            json!({"ResourceArn": &table_arn, "Policy": json!({
                "Version": "2012-10-17",
                "Statement": [{"Effect": "Allow", "Principal": "*",
                               "Action": "dynamodb:GetItem", "Resource": &table_arn}],
            }).to_string()}),
            "One or more parameter values were invalid: Invalid policy document: Policy \
             statement must not grant access to a public principal"
                .to_string(),
        ),
        (
            "a truncated document",
            json!({"ResourceArn": &table_arn,
                   "Policy": "{\"Version\": \"2012-10-17\", \"Statement\": [{\"Effect\""}),
            "One or more parameter values were invalid: Invalid policy document: This policy \
             contains invalid Json"
                .to_string(),
        ),
        (
            "an index ARN",
            json!({"ResourceArn": format!("{table_arn}/index/by-rank"),
                   "Policy": &stream_policy}),
            format!(
                "One or more parameter values were invalid: Invalid resource arn provided, only \
                 table or stream is accepted. Provided resource arn: {table_arn}/index/by-rank"
            ),
        ),
        (
            "another region's table ARN",
            json!({"ResourceArn": format!("arn:aws:dynamodb:eu-west-1:{ACCOUNT}:table/{table}"),
                   "Policy": &table_policy}),
            format!(
                "One or more parameter values were invalid: Invalid resource Arn: \
                 arn:aws:dynamodb:eu-west-1:{ACCOUNT}:table/{table}"
            ),
        ),
    ];
    for (case, request, expected) in &rows {
        let (status, answered) = call(&router, "PutResourcePolicy", request.clone()).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
        );
        assert_eq!(
            &message(&answered),
            expected,
            "case {case:?}: expected {expected:?}, got {}",
            message(&answered)
        );
    }
    let kept = ok(&router, "GetResourcePolicy", json!({"ResourceArn": &table_arn})).await;
    assert_eq!(
        kept["Policy"],
        json!(&table_policy),
        "case after every refusal: the stored policy is the one that was attached, got {kept}"
    );
}

#[tokio::test]
async fn a_policy_document_is_stored_as_the_request_stated_it() {
    let router = emulator();
    let table = "PolicyVersions";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        }),
    )
    .await;
    let described = crate::emulator::describe_when_active(&router, "policy versions", table).await;
    let arn = described["TableArn"].as_str().unwrap_or_default().to_string();

    // Rows differ along the version the document states: a document that states one
    // keeps it, and one that states none is stored and reported WITHOUT one — the
    // `Version` such a document is read under ("if you do not include a Version element,
    // the value defaults to 2008-10-17") is a reading of the document, not a member of
    // it, so nothing is added to what the request stated.
    for (case, document, expected_version) in [
        (
            "a document with no Version",
            json!({"Statement": [{"Effect": "Deny", "Principal": {"AWS": "*"},
                                  "Action": "dynamodb:GetItem", "Resource": &arn}]}),
            Value::Null,
        ),
        (
            "a document that states one",
            json!({"Version": "2012-10-17",
                   "Statement": [{"Effect": "Deny", "Principal": {"AWS": "*"},
                                  "Action": "dynamodb:PutItem", "Resource": &arn}]}),
            json!("2012-10-17"),
        ),
    ] {
        ok(
            &router,
            "PutResourcePolicy",
            json!({"ResourceArn": &arn, "Policy": document.to_string(),
                   "ConfirmRemoveSelfResourceAccess": true}),
        )
        .await;
        // The attach is asynchronous, so the document is read back once it is applied.
        wait_for_policy_application().await;
        let read = ok(&router, "GetResourcePolicy", json!({"ResourceArn": &arn})).await;
        assert_eq!(
            read["Policy"],
            json!(document.to_string()),
            "case {case:?}: expected the document the request stated, got {read}"
        );
        let stored: Value = serde_json::from_str(read["Policy"].as_str().unwrap_or("null"))
            .unwrap_or(Value::Null);
        assert_eq!(
            stored["Version"],
            expected_version,
            "case {case:?}: expected the stored document to state Version \
             {expected_version:?}, got {stored}"
        );
        assert_eq!(
            stored["Statement"],
            document["Statement"],
            "case {case:?}: the statements are stored exactly as they arrived, got {stored}"
        );
    }
}

#[tokio::test]
async fn a_second_create_of_one_name_is_refused_by_the_state_the_table_is_in() {
    let router = emulator();
    let table = "RacedCreate";
    let create = json!({
        "TableName": table,
        "BillingMode": "PAY_PER_REQUEST",
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
    });
    ok(&router, "CreateTable", create.clone()).await;

    // While the creation the clock owes has not fallen due, the table is a resource
    // still in use, which is what every loser of a race against one creator is told.
    let (status, answered) = call(&router, "CreateTable", create.clone()).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ResourceInUseException"),
        "case a create against a table still being created: expected ResourceInUseException, \
         got {status} with {answered}"
    );
    assert_eq!(
        message(&answered),
        format!(
            "Attempt to change a resource which is still in use: Table is being created: {table}"
        ),
        "case a create against a table still being created: got {}",
        message(&answered)
    );

    // Once the table has settled, the name is simply taken.
    crate::emulator::describe_when_active(&router, "raced create", table).await;
    let (status, answered) = call(&router, "CreateTable", create).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ResourceInUseException"),
        "case a create against a settled table: expected ResourceInUseException, got {status} \
         with {answered}"
    );
    assert_eq!(
        message(&answered),
        format!("Table already exists: {table}"),
        "case a create against a settled table: got {}",
        message(&answered)
    );
}

#[tokio::test]
async fn a_schema_defect_is_worded_over_the_request_and_the_tables_own_schema() {
    let router = emulator();

    // Rows differ along the defect: a key schema whose elements are out of order, a key
    // attribute with no definition (on the table's own key and on an index's), the
    // billing mode's throughput pair both ways round, and a vector index asking for more
    // dimensions than one can hold.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "an index key schema with a partition key after its sort key",
            json!({
                "TableName": "OutOfOrder",
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "pk", "AttributeType": "S"},
                    {"AttributeName": "a", "AttributeType": "S"},
                    {"AttributeName": "b", "AttributeType": "S"},
                    {"AttributeName": "c", "AttributeType": "S"},
                ],
                "GlobalSecondaryIndexes": [{
                    "IndexName": "idx",
                    "KeySchema": [{"AttributeName": "a", "KeyType": "HASH"},
                                  {"AttributeName": "b", "KeyType": "RANGE"},
                                  {"AttributeName": "c", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                }],
            }),
            "One or more parameter values were invalid: All HASH key attributes must precede \
             RANGE key attributes in the KeySchema"
                .to_string(),
        ),
        (
            // Every definition is used, so the key with nothing declared for it is the
            // only fact there is to state, and the sentence interpolates nothing.
            "a table key attribute with no definition",
            json!({
                "TableName": "UndefinedTableKey",
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                              {"AttributeName": "sk", "KeyType": "RANGE"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            }),
            "Invalid KeySchema: Some index key attribute have no definition".to_string(),
        ),
        (
            // The two sets differ BOTH ways round — a key with no definition beside a
            // definition no key schema uses — so each list is reported, in the order a
            // name-keyed table renders it rather than the one the request stated.
            "a table key attribute with no definition beside an unused definition",
            json!({
                "TableName": "UndefinedTableKeyAndExtra",
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "SK", "KeyType": "HASH"},
                              {"AttributeName": "PK", "KeyType": "RANGE"}],
                "AttributeDefinitions": [{"AttributeName": "EXTRA", "AttributeType": "S"},
                                         {"AttributeName": "PK", "AttributeType": "S"}],
            }),
            "One or more parameter values were invalid: Some index key attributes are not \
             defined in AttributeDefinitions. Keys: [SK, PK], AttributeDefinitions: [EXTRA, PK]"
                .to_string(),
        ),
        (
            // Neither list is reported in the order the request stated it: both are
            // rendered out of a name-keyed table, so the position of a name is decided by
            // the name alone.
            "an index key attribute with no definition",
            json!({
                "TableName": "UndefinedIndexKey",
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "PK", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "PK", "AttributeType": "S"},
                                         {"AttributeName": "EXTRA", "AttributeType": "S"}],
                "GlobalSecondaryIndexes": [
                    {
                        "IndexName": "idx",
                        "KeySchema": [{"AttributeName": "PK", "KeyType": "HASH"},
                                      {"AttributeName": "SK", "KeyType": "RANGE"}],
                        "Projection": {"ProjectionType": "ALL"},
                    },
                    {
                        "IndexName": "extra",
                        "KeySchema": [{"AttributeName": "EXTRA", "KeyType": "HASH"}],
                        "Projection": {"ProjectionType": "ALL"},
                    },
                ],
            }),
            "One or more parameter values were invalid: Some index key attributes are not \
             defined in AttributeDefinitions. Keys: [SK, PK], AttributeDefinitions: \
             [EXTRA, PK]"
                .to_string(),
        ),
        (
            "an on-demand table that also states a provisioned throughput",
            json!({
                "TableName": "OnDemandWithThroughput",
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            }),
            "One or more parameter values were invalid: Neither ReadCapacityUnits nor \
             WriteCapacityUnits can be specified when BillingMode is PAY_PER_REQUEST"
                .to_string(),
        ),
        (
            "a provisioned table that states none",
            json!({
                "TableName": "ProvisionedWithout",
                "BillingMode": "PROVISIONED",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            }),
            "One or more parameter values were invalid: ProvisionedThroughput must be specified \
             when BillingMode is PROVISIONED"
                .to_string(),
        ),
        (
            "a vector index past the dimension ceiling",
            json!({
                "TableName": "OverDimensioned",
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                "VectorIndexes": [{
                    "IndexName": "vectors",
                    "VectorAttribute": {"AttributeName": "emb"},
                    "Dimensions": 4097,
                    "DistanceFunction": "COSINE",
                    "Projection": {"ProjectionType": "ALL"},
                }],
            }),
            "One or more parameter values were invalid: Number of dimensions must be between 1 \
             and 4096 inclusive."
                .to_string(),
        ),
    ];

    for (case, request, expected) in &rows {
        let (status, answered) = call(&router, "CreateTable", request.clone()).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
        );
        assert_eq!(
            &message(&answered),
            expected,
            "case {case:?}: expected {expected:?}, got {}",
            message(&answered)
        );
        let (status, listed) = call(&router, "ListTables", json!({})).await;
        assert_eq!(status, 200, "case {case:?}: ListTables answered {listed}");
        let names = listed["TableNames"].as_array().cloned().unwrap_or_default();
        assert!(
            !names.contains(&request["TableName"]),
            "case {case:?}: a refused create stores no table, got {listed}"
        );
    }

    // The control: the ceiling's own value is accepted, and the index reports it.
    let created = ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "AtTheCeiling",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "VectorIndexes": [{
                "IndexName": "vectors",
                "VectorAttribute": {"AttributeName": "emb"},
                "Dimensions": 4096,
                "DistanceFunction": "COSINE",
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    assert_eq!(
        created["TableDescription"]["VectorIndexes"][0]["Dimensions"],
        json!(4096),
        "case the ceiling itself: expected the index to report 4096 dimensions, got {created}"
    );
}

#[tokio::test]
async fn a_write_whose_vector_is_not_the_indexed_shape_is_refused() {
    let router = emulator();
    let table = "VectorWrites";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "VectorIndexes": [{
                "IndexName": "vectors",
                "VectorAttribute": {"AttributeName": "emb"},
                "Dimensions": 3,
                "DistanceFunction": "COSINE",
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    crate::emulator::describe_when_active(&router, "vector writes", table).await;

    // Rows differ along HOW the written vector misses the shape the index was built for:
    // too few values, too many, a value that is not a number, and a value that is not a
    // list at all.
    let rows: Vec<(&str, Value)> = vec![
        ("one value short", json!({"L": [{"N": "1"}, {"N": "0"}]})),
        (
            "one value too many",
            json!({"L": [{"N": "1"}, {"N": "0"}, {"N": "0"}, {"N": "0"}]}),
        ),
        (
            "a member that is not a number",
            json!({"L": [{"N": "1"}, {"S": "0"}, {"N": "0"}]}),
        ),
        ("a value that is not a list", json!({"S": "1,0,0"})),
    ];
    for (case, embedding) in &rows {
        let (status, answered) = call(
            &router,
            "PutItem",
            json!({"TableName": table,
                   "Item": {"pk": {"S": "item"}, "emb": embedding}}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
        );
    }

    // The control: a vector of the index's own shape is written and reads back, and an
    // item carrying no vector at all is simply not covered by the index.
    for (case, item) in [
        (
            "a vector of the index's shape",
            json!({"pk": {"S": "item"}, "emb": {"L": [{"N": "1"}, {"N": "0"}, {"N": "0"}]}}),
        ),
        ("no vector at all", json!({"pk": {"S": "plain"}})),
    ] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": &item}),
        )
        .await;
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": table, "Key": {"pk": item["pk"].clone()}}),
        )
        .await;
        assert_eq!(
            read["Item"], item,
            "case {case:?}: the stored item reads back as it was written, got {read}"
        );
    }
}

#[tokio::test]
async fn one_update_carries_one_index_lifecycle_action_and_names_the_index_it_deletes() {
    let router = emulator();
    let table = "IndexLifecycle";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"},
                                     {"AttributeName": "rank", "AttributeType": "N"}],
            "GlobalSecondaryIndexes": [{
                "IndexName": "ByRank",
                "KeySchema": [{"AttributeName": "rank", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    crate::emulator::describe_when_active(&router, "index lifecycle", table).await;

    // An on-demand table is refused a provisioned throughput: a request that names the
    // billing mode too is told the two members contradict each other.
    let (status, answered) = call(
        &router,
        "UpdateTable",
        json!({"TableName": table,
               "BillingMode": "PAY_PER_REQUEST",
               "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5}}),
    )
    .await;
    assert_eq!(
        (
            status,
            error_code(&answered).as_str(),
            message(&answered).as_str()
        ),
        (
            400,
            "ValidationException",
            "One or more parameter values were invalid: ProvisionedThroughput cannot be \
             specified when BillingMode is PAY_PER_REQUEST"
        ),
        "case a throughput beside the billing mode that rules it out: got {answered}"
    );
    // A request that names only the throughput is told what the mode in force accepts.
    let (status, answered) = call(
        &router,
        "UpdateTable",
        json!({"TableName": table,
               "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5}}),
    )
    .await;
    assert_eq!(
        (
            status,
            error_code(&answered).as_str(),
            message(&answered).as_str()
        ),
        (
            400,
            "ValidationException",
            "One or more parameter values were invalid: Neither ReadCapacityUnits nor \
             WriteCapacityUnits can be specified when BillingMode is PAY_PER_REQUEST"
        ),
        "case a throughput on an on-demand table: got {answered}"
    );

    // A create and a delete in ONE call are two lifecycle actions on one table's indexes,
    // which is one more than a call may carry: "You can create or delete only one global
    // secondary index per UpdateTable operation" (`UpdateTable$GlobalSecondaryIndexUpdates`).
    let (status, answered) = call(
        &router,
        "UpdateTable",
        json!({
            "TableName": table,
            "AttributeDefinitions": [{"AttributeName": "label", "AttributeType": "S"}],
            "GlobalSecondaryIndexUpdates": [
                {"Create": {"IndexName": "ByLabel",
                            "KeySchema": [{"AttributeName": "label", "KeyType": "HASH"}],
                            "Projection": {"ProjectionType": "ALL"}}},
                {"Delete": {"IndexName": "ByRank"}},
            ],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "LimitExceededException"),
        "case a create and a delete in one call: expected the control plane's limit error, got \
         {status} with {answered}"
    );

    // Deleting an index the table carries none of addresses the INDEX as the resource it
    // removes, so it is the control plane's lookup miss over both names.
    for absent in ["by-nothing", "ByRankk"] {
        let (status, answered) = call(
            &router,
            "UpdateTable",
            json!({"TableName": table,
                   "GlobalSecondaryIndexUpdates": [{"Delete": {"IndexName": absent}}]}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ResourceNotFoundException"),
            "case deleting the absent index {absent:?}: expected 400 ResourceNotFoundException, \
             got {status} with {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("Requested resource not found: Index {absent} for table {table}"),
            "case deleting the absent index {absent:?}: got {}",
            message(&answered)
        );
    }

    // The control: one delete of an index the table DOES carry is applied, and the index
    // is gone from the description.
    ok(
        &router,
        "UpdateTable",
        json!({"TableName": table,
               "GlobalSecondaryIndexUpdates": [{"Delete": {"IndexName": "ByRank"}}]}),
    )
    .await;
    let described = ok(&router, "DescribeTable", json!({"TableName": table})).await;
    assert_eq!(
        crate::emulator::index_named(&described["Table"], "ByRank"),
        Value::Null,
        "case the delete of an index the table carries: expected it gone, got {described}"
    );
    let _ = REGION;
}

#[tokio::test]
async fn a_secondary_index_carries_the_warm_throughput_it_was_created_with() {
    let router = emulator();
    let table = "IndexWarmThroughput";
    // Rows differ along what the rule generalises over: whether the request names the
    // warm throughput at all, and whether it names one half of the pair or both.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"},
                                     {"AttributeName": "rank", "AttributeType": "N"},
                                     {"AttributeName": "label", "AttributeType": "S"}],
            "GlobalSecondaryIndexes": [
                {"IndexName": "Named",
                 "KeySchema": [{"AttributeName": "rank", "KeyType": "HASH"}],
                 "Projection": {"ProjectionType": "ALL"},
                 "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
                 "WarmThroughput": {"ReadUnitsPerSecond": 40, "WriteUnitsPerSecond": 20}},
                {"IndexName": "Unnamed",
                 "KeySchema": [{"AttributeName": "label", "KeyType": "HASH"}],
                 "Projection": {"ProjectionType": "ALL"},
                 "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 4}},
            ],
        }),
    )
    .await;
    let described = crate::emulator::describe_when_active(&router, "warm throughput", table).await;
    for (case, index, read_units, write_units) in [
        ("the index that named the pair", "Named", 40, 20),
        // An index that names none keeps warm the capacity it is provisioned for, by the
        // same derivation the table's own warm throughput follows.
        ("the index that named none", "Unnamed", 3, 4),
    ] {
        let reported = crate::emulator::index_named(&described, index);
        assert_ne!(
            reported,
            Value::Null,
            "case {case:?}: DescribeTable must report the index it was created with, got \
             {described}"
        );
        assert_eq!(
            reported["IndexStatus"],
            json!("ACTIVE"),
            "case {case:?}: the index reaches ACTIVE with its table, got {reported}"
        );
        assert_eq!(
            (
                reported["WarmThroughput"]["ReadUnitsPerSecond"].clone(),
                reported["WarmThroughput"]["WriteUnitsPerSecond"].clone(),
                reported["WarmThroughput"]["Status"].clone(),
            ),
            (json!(read_units), json!(write_units), json!("ACTIVE")),
            "case {case:?}: expected the warm throughput in force, got {reported}"
        );
    }

    // The same member on an index ADDED later is stored and reported too.
    ok(
        &router,
        "UpdateTable",
        json!({"TableName": table,
               "AttributeDefinitions": [{"AttributeName": "added", "AttributeType": "S"}],
               "GlobalSecondaryIndexUpdates": [{"Create": {
                   "IndexName": "Added",
                   "KeySchema": [{"AttributeName": "added", "KeyType": "HASH"}],
                   "Projection": {"ProjectionType": "KEYS_ONLY"},
                   "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
                   "WarmThroughput": {"ReadUnitsPerSecond": 25, "WriteUnitsPerSecond": 15}}}]}),
    )
    .await;
    let settled = crate::emulator::describe_when_active(&router, "added index", table).await;
    let added = crate::emulator::index_named(&settled, "Added");
    assert_eq!(
        (
            added["IndexStatus"].clone(),
            added["WarmThroughput"]["ReadUnitsPerSecond"].clone(),
            added["WarmThroughput"]["WriteUnitsPerSecond"].clone(),
        ),
        (json!("ACTIVE"), json!(25), json!(15)),
        "case an index added later: expected it ACTIVE with the warm throughput it was \
         created with, got {settled}"
    );
}

#[tokio::test]
async fn a_vector_write_meters_the_bytes_the_index_stores_with_its_documented_floor() {
    let router = emulator();
    // Rows differ along the index's Dimensions, which is what the per-dimension cost is
    // computed from: a vector small enough that the floor decides the figure, and one
    // whose own bytes carry it well past the floor.
    let cases: [(&str, &str, i64); 2] = [
        ("a small vector", "vector-floor", 3),
        ("a vector of many dimensions", "vector-large", 2_000),
    ];
    let mut metered: Vec<f64> = Vec::new();
    for (case, table, dimensions) in cases {
        ok(
            &router,
            "CreateTable",
            json!({
                "TableName": table,
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                "VectorIndexes": [{
                    "IndexName": "vectors",
                    "VectorAttribute": {"AttributeName": "emb"},
                    "Dimensions": dimensions,
                    "DistanceFunction": "COSINE",
                    "Projection": {"ProjectionType": "ALL"},
                }],
            }),
        )
        .await;
        crate::emulator::describe_when_active(&router, case, table).await;
        let embedding: Vec<Value> = (0..dimensions).map(|_| json!({"N": "0.5"})).collect();
        let written = ok(
            &router,
            "PutItem",
            json!({"TableName": table,
                   "Item": {"pk": {"S": "item"}, "emb": {"L": embedding}},
                   "ReturnConsumedCapacity": "INDEXES"}),
        )
        .await;
        let bytes = written["ConsumedCapacity"]["VectorIndexes"]["vectors"]
            ["VectorWriteRequestBytes"]
            .as_f64();
        let floor: f64 = 1024.0;
        let expected_least = floor.max((dimensions * 4) as f64);
        let Some(bytes) = bytes else {
            panic!(
                "case {case:?}: a write to a table with a vector index must meter it, got {written}"
            );
        };
        assert!(
            bytes >= expected_least,
            "case {case:?}: expected at least {expected_least} bytes (the 1 KB floor and four \
             bytes per dimension), got {bytes}"
        );
        if dimensions * 4 < 1024 {
            assert_eq!(
                bytes, floor,
                "case {case:?}: a vector below the floor meters the floor itself, got {bytes}"
            );
        }
        metered.push(bytes);

        // An item that carries no vector is not covered by the index, so the write
        // meters nothing on it.
        let plain = ok(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": {"pk": {"S": "plain"}},
                   "ReturnConsumedCapacity": "INDEXES"}),
        )
        .await;
        assert_eq!(
            plain["ConsumedCapacity"].get("VectorIndexes"),
            None,
            "case {case:?}: an item with no vector reaches no vector index, got {plain}"
        );
    }
    assert!(
        metered.last() > metered.first(),
        "case the per-dimension cost: more dimensions must meter more bytes, got {metered:?}"
    );
}
