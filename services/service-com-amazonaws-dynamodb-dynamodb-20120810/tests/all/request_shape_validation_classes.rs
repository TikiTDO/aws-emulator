// SPDX-License-Identifier: CC0-1.0

//! Four classes of REQUEST-SHAPE refusal, each driven end to end over the wire.
//!
//! Every suite here is a TABLE spanning what its rule generalises over rather than one
//! recorded request, and every assertion names its case and the value it actually read.
//!
//! RULE 1 — a `RequestItems` map KEY that satisfies none of the constraints a table name
//! is held to is refused as a map key, and the clause QUOTES the map that key sat in, as
//! the JSON the request carried it as: a request-validator clause states the value it
//! refused, which is the only place the offending map is reported. The rows span the way
//! the key breaks the constraints, the members the entry carried, the datatypes its keys
//! hold, the number of entries in the map, and a value whose characters JSON escapes.
//!
//! RULE 2 — the read/write capacity mode a table is created or updated with is stated
//! through either of the two members the model declares for it, `BillingMode` and
//! `TableThroughputMode` (`smithy_query(query="operation", operation="CreateTable")`), both
//! targeting the `BillingMode` enum, so a value outside that set is refused in the
//! request-shape pass — the rejected value echoed, the path of the member that carried it,
//! and the whole admissible set — BEFORE any capacity semantics are evaluated and before
//! the table is looked up, and a value INSIDE it sets the mode whichever member carried it.
//! The rows span the member, the form of the rejected value, the operation that carries it,
//! and what else is wrong with the same request. The boundary rows are the capacity
//! sentences the mode is then held to, which the create and the update state in the same
//! words.
//!
//! RULE 3 — a substitution table (`ExpressionAttributeNames`/`ExpressionAttributeValues`)
//! sent by a request that names no expression THAT TABLE can be substituted into is refused
//! with the sentence that says to add an expression, which is a different defect from
//! entries an expression that IS present does not reference (the sentence that says to drop
//! the entry). A `#alias` goes into any expression member; a `:placeholder` goes only into
//! one that compares or writes a value, so a projection is not one. Whether the sentence
//! carries the `<N> validation error(s) detected: ` envelope follows the rule every refusal
//! about a request's expressions follows: the members a WRITE states its intent through
//! carry it, the members a READ narrows its result with do not, and an ELEMENT's refusal is
//! stated outside it. The rows span the table, the operation, the expression member the
//! request did carry, and whether the members belong to the request or to one of its
//! elements.
//!
//! RULE 4 — a `SELECT` projection list reports one entry per path it names, keyed by the
//! identifier that path is reported under (its alias, else its last named step), so a list
//! that states one identifier twice asks for an item in which one key carries two values
//! and is refused naming the repeated identifier(s). The rows span the ways two items
//! collapse onto one identifier, and the controls span the lists that do not.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

// ===========================================================================
// RULE 1. A refused RequestItems key quotes the map it sat in
// ===========================================================================

/// The table the batched rows address when they address a real one.
const BATCHED: &str = "Batched";

/// The whole message a refused `RequestItems` key earns, over the map the clause quotes.
///
/// The constraint set is the one a table name is held to (3..=255 and `[a-zA-Z0-9_.-]+`),
/// which the model does not carry on the map's key shape
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#BatchGetRequestMap")` gives
/// a key of `TableArn`, length 1..1024).
fn map_key_clause(quoted: Option<&str>) -> String {
    let value = match quoted {
        Some(rendered) => format!("Value '{rendered}' at"),
        None => "Value at".to_string(),
    };
    format!(
        "1 validation error detected: {value} 'requestItems' failed to satisfy constraint: Map \
         keys must satisfy constraint: [Member must have length less than or equal to 255, \
         Member must have length greater than or equal to 3, Member must satisfy regular \
         expression pattern: [a-zA-Z0-9_.-]+]"
    )
}

#[tokio::test]
async fn a_refused_request_items_key_quotes_the_map_the_request_sent() {
    let router = emulator();
    create_active_table(&router, BATCHED, &[("pk", "HASH", "S")]).await;
    let long_key = "k".repeat(256);

    // (case, the RequestItems map the request sends, the JSON the clause must quote)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a key below the minimum name length, one key, one attribute",
            json!({"ab": {"Keys": [{"pk": {"S": "a"}}]}}),
            r#"{"ab":{"Keys":[{"pk":{"S":"a"}}]}}"#.to_string(),
        ),
        (
            "a key outside the name pattern, two keys",
            json!({"bad name!": {"Keys": [{"pk": {"S": "a"}}, {"pk": {"S": "b"}}]}}),
            r#"{"bad name!":{"Keys":[{"pk":{"S":"a"}},{"pk":{"S":"b"}}]}}"#.to_string(),
        ),
        (
            "a key past the maximum name length, with every other entry member sent",
            json!({long_key.clone(): {"Keys": [{"pk": {"S": "a"}}],
                                      "ConsistentRead": true,
                                      "ProjectionExpression": "#p",
                                      "ExpressionAttributeNames": {"#p": "pk"}}}),
            format!(
                "{{{:?}:{{\"Keys\":[{{\"pk\":{{\"S\":\"a\"}}}}],\"ConsistentRead\":true,\
                 \"ProjectionExpression\":\"#p\",\"ExpressionAttributeNames\":{{\"#p\":\"pk\"}}}}}}",
                long_key
            ),
        ),
        (
            "a key that is an ARN of another service, beside an entry that names a table",
            json!({format!("arn:aws:sqs:us-east-1:000000000000:{BATCHED}"):
                       {"Keys": [{"pk": {"S": "b"}}]},
                   BATCHED: {"Keys": [{"pk": {"S": "a"}}]}}),
            format!(
                "{{\"{BATCHED}\":{{\"Keys\":[{{\"pk\":{{\"S\":\"a\"}}}}]}},\
                 \"arn:aws:sqs:us-east-1:000000000000:{BATCHED}\":\
                 {{\"Keys\":[{{\"pk\":{{\"S\":\"b\"}}}}]}}}}"
            ),
        ),
        (
            "a key of a datatype each, and the legacy attribute list",
            json!({"ab": {"Keys": [{"pk": {"S": "a"}, "n": {"N": "1"},
                                    "b": {"B": "AQ=="}, "ss": {"SS": ["x", "y"]},
                                    "l": {"L": [{"S": "p"}, {"NULL": true}]},
                                    "m": {"M": {"inner": {"BOOL": false}}}}],
                          "AttributesToGet": ["pk", "n"]}}),
            concat!(
                r#"{"ab":{"Keys":[{"b":{"B":"AQ=="},"l":{"L":[{"S":"p"},{"NULL":true}]},"#,
                r#""m":{"M":{"inner":{"BOOL":false}}},"n":{"N":"1"},"pk":{"S":"a"},"#,
                r#""ss":{"SS":["x","y"]}}],"AttributesToGet":["pk","n"]}}"#
            )
            .to_string(),
        ),
        (
            "a key value whose characters JSON escapes",
            json!({"ab": {"Keys": [{"pk": {"S": "a\"b\\c\nd"}}]}}),
            "{\"ab\":{\"Keys\":[{\"pk\":{\"S\":\"a\\\"b\\\\c\\nd\"}}]}}".to_string(),
        ),
    ];

    for (case, request_items, quoted) in rows {
        let (status, body) = call(
            &router,
            "BatchGetItem",
            json!({"RequestItems": request_items}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {body}"
        );
        let said = message(&body);
        let expected = map_key_clause(Some(&quoted));
        assert_eq!(
            said, expected,
            "case {case:?}: expected {expected:?}, got {said:?}"
        );
    }

    // The OTHER rendering: `BatchWriteItem`'s clause quotes the same map in the OBJECT
    // form the service's own request validator states a refused structure value with, over
    // the fields of its own request objects — which is what the recorded refusals of that
    // member carry (`crate::state::rendered_batch_write_request_items`), and not the JSON
    // the request arrived as.
    let (status, body) = call(
        &router,
        "BatchWriteItem",
        json!({"RequestItems": {"ab": [{"PutRequest": {"Item": {"pk": {"S": "a"}}}}]}}),
    )
    .await;
    let said = message(&body);
    let expected = map_key_clause(Some(
        "{ab=[WriteRequest(putRequest=PutRequest(item={pk=AttributeValue(s=a, n=null, b=null, \
         iNT=null, dECIMAL=null, dOUBLE=null, sS=null, nS=null, bS=null, iNTSET=null, \
         dECIMALSET=null, dOUBLESET=null, m=null, l=null, nULL=null, bOOL=null)}, \
         workloadProfileName=null), deleteRequest=null)]}",
    ));
    assert_eq!(
        (status, said.as_str()),
        (400, expected.as_str()),
        "case a batched WRITE: expected {expected:?}, got {status} with {body}"
    );

    // Both spellings a key accepts still address the table, so the refusals above are
    // about keys that name no table at all.
    for (case, key) in [
        ("the bare table name", BATCHED.to_string()),
        (
            "the table's own ARN",
            format!("arn:aws:dynamodb:us-east-1:000000000000:table/{BATCHED}"),
        ),
    ] {
        let served = ok(
            &router,
            "BatchGetItem",
            json!({"RequestItems": {key.clone(): {"Keys": [{"pk": {"S": "a"}}]}}}),
        )
        .await;
        assert!(
            served["Responses"][&key].is_array(),
            "case {case:?}: expected the entry to be served, got {served}"
        );
    }
}

// ===========================================================================
// RULE 2. A BillingMode outside the declared value set
// ===========================================================================

/// The table the `UpdateTable` rows address.
const MODED: &str = "Moded";

/// The whole message a capacity mode outside the declared set earns: the value echoed from
/// the request, the path of the member that carried it, and the set in the order the model
/// declares it (`smithy_query(query="shape",
/// shape="com.amazonaws.dynamodb#BillingMode")`).
fn throughput_mode_clause(value: &str, path: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy constraint: \
         Member must satisfy enum value set: [PROVISIONED, PAY_PER_REQUEST]"
    )
}

/// A `CreateTable` body for a table keyed on `pk`, with `extra` merged in.
fn create_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
    });
    for (member, value) in extra.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    body
}

#[tokio::test]
async fn a_capacity_mode_outside_the_declared_set_is_refused_as_the_member_it_is() {
    let router = emulator();
    create_active_table(&router, MODED, &[("pk", "HASH", "S")]).await;

    // (case, operation, request, the value the clause must echo, the member path it names).
    // Both members the model declares for the mode are spanned, because a request states
    // it through either one.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a mode no release names, through the throughput-mode member, with no capacity",
            "CreateTable",
            create_body("created-1", json!({"TableThroughputMode": "BOGUS_MODE"})),
            "BOGUS_MODE",
            "tableThroughputMode",
        ),
        (
            "the same value through the billing-mode member",
            "CreateTable",
            create_body("created-2", json!({"BillingMode": "BOGUS_MODE"})),
            "BOGUS_MODE",
            "billingMode",
        ),
        (
            "a declared mode in the wrong case, with capacity supplied",
            "CreateTable",
            create_body(
                "created-3",
                json!({"TableThroughputMode": "provisioned",
                       "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                                 "WriteCapacityUnits": 1}}),
            ),
            "provisioned",
            "tableThroughputMode",
        ),
        (
            "an empty mode",
            "CreateTable",
            create_body("created-4", json!({"TableThroughputMode": ""})),
            "",
            "tableThroughputMode",
        ),
        (
            "a mode taken from another enum of the same model",
            "CreateTable",
            create_body("created-5", json!({"BillingMode": "STANDARD"})),
            "STANDARD",
            "billingMode",
        ),
        (
            "a plausible newer mode name",
            "CreateTable",
            create_body("created-6", json!({"TableThroughputMode": "ON_DEMAND"})),
            "ON_DEMAND",
            "tableThroughputMode",
        ),
        (
            "the mode clause wins over capacity the resolved mode could not take",
            "CreateTable",
            create_body(
                "created-7",
                json!({"TableThroughputMode": "BOGUS_MODE",
                       "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                                 "WriteCapacityUnits": 1}}),
            ),
            "BOGUS_MODE",
            "tableThroughputMode",
        ),
        (
            "the mode clause wins over a key schema that names an undeclared attribute",
            "CreateTable",
            json!({"TableName": "created-8",
                   "KeySchema": [{"AttributeName": "absent", "KeyType": "HASH"}],
                   "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                   "TableThroughputMode": "BOGUS_MODE"}),
            "BOGUS_MODE",
            "tableThroughputMode",
        ),
        (
            "the throughput-mode member on an update of a table that exists",
            "UpdateTable",
            json!({"TableName": MODED, "TableThroughputMode": "BOGUS_MODE"}),
            "BOGUS_MODE",
            "tableThroughputMode",
        ),
        (
            "a declared mode in the wrong case, on an update",
            "UpdateTable",
            json!({"TableName": MODED, "BillingMode": "pay_per_request"}),
            "pay_per_request",
            "billingMode",
        ),
        (
            "the mode clause wins over the table not being there at all",
            "UpdateTable",
            json!({"TableName": "never-created", "TableThroughputMode": "BOGUS_MODE"}),
            "BOGUS_MODE",
            "tableThroughputMode",
        ),
    ];

    for (case, operation, request, value, path) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
             {body}"
        );
        let said = message(&body);
        let expected = throughput_mode_clause(value, path);
        assert_eq!(
            said, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {said:?}"
        );
    }

    // A request that breaks BOTH members earns one clause per member, counted by the
    // envelope and stated in the order the members are read: the billing-mode member
    // first.
    for (case, operation, request) in [
        (
            "both mode members outside the set, on a create",
            "CreateTable",
            create_body(
                "created-9",
                json!({"BillingMode": "ALSO_BOGUS", "TableThroughputMode": "BOGUS_MODE"}),
            ),
        ),
        (
            "both mode members outside the set, on an update",
            "UpdateTable",
            json!({"TableName": MODED, "BillingMode": "ALSO_BOGUS",
                   "TableThroughputMode": "BOGUS_MODE"}),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        let said = message(&body);
        let expected = format!(
            "2 validation errors detected: {}; {}",
            throughput_mode_clause("ALSO_BOGUS", "billingMode")
                .trim_start_matches("1 validation error detected: "),
            throughput_mode_clause("BOGUS_MODE", "tableThroughputMode")
                .trim_start_matches("1 validation error detected: ")
        );
        assert_eq!(
            (status, said.as_str()),
            (400, expected.as_str()),
            "case {case:?} ({operation}): expected {expected:?}, got {status} with {body}"
        );
    }

    // The BOUNDARY: once the mode IS one the set admits, what the request says about
    // capacity is judged — the two sentences that hold a mode and its capacity together,
    // each stated once by the operations that share them.
    for (case, operation, request, expected) in [
        (
            "a provisioned create with no capacity at all",
            "CreateTable",
            create_body("capacity-1", json!({"TableThroughputMode": "PROVISIONED"})),
            "One or more parameter values were invalid: ProvisionedThroughput must be specified \
             when BillingMode is PROVISIONED",
        ),
        (
            "an on-demand create that names capacity anyway",
            "CreateTable",
            create_body(
                "capacity-2",
                json!({"TableThroughputMode": "PAY_PER_REQUEST",
                       "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                                 "WriteCapacityUnits": 1}}),
            ),
            "One or more parameter values were invalid: Neither ReadCapacityUnits nor \
             WriteCapacityUnits can be specified when BillingMode is PAY_PER_REQUEST",
        ),
        (
            "an on-demand update that names both the mode and capacity",
            "UpdateTable",
            json!({"TableName": MODED, "TableThroughputMode": "PAY_PER_REQUEST",
                   "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                             "WriteCapacityUnits": 1}}),
            "One or more parameter values were invalid: ProvisionedThroughput cannot be \
             specified when BillingMode is PAY_PER_REQUEST",
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        let said = message(&body);
        assert_eq!(
            (status, said.as_str()),
            (400, expected),
            "case {case:?} ({operation}): expected {expected:?}, got {status} with {body}"
        );
    }

    // Nothing above was created: a refused create registers no table.
    for name in ["created-1", "created-2", "created-3", "created-4", "created-5",
                 "created-6", "created-7", "created-8", "created-9",
                 "capacity-1", "capacity-2"] {
        let (status, body) = call(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a refused create must register no table, got {status} with {body}"
        );
    }

    // The CONTROL: each declared value is served through either member, and the table
    // reports the mode it was created or updated with.
    let provisioned = ok(
        &router,
        "CreateTable",
        create_body(
            "provisioned-table",
            json!({"BillingMode": "PROVISIONED",
                   "ProvisionedThroughput": {"ReadCapacityUnits": 2,
                                             "WriteCapacityUnits": 3}}),
        ),
    )
    .await;
    assert_eq!(
        provisioned["TableDescription"]["ProvisionedThroughput"]["ReadCapacityUnits"],
        json!(2),
        "case \"a provisioned create\": expected the capacity it asked for, got {provisioned}"
    );
    let on_demand = ok(
        &router,
        "CreateTable",
        create_body("on-demand-table", json!({"BillingMode": "PAY_PER_REQUEST"})),
    )
    .await;
    assert_eq!(
        on_demand["TableDescription"]["BillingModeSummary"]["BillingMode"],
        json!("PAY_PER_REQUEST"),
        "case \"an on-demand create\": expected the mode it asked for, got {on_demand}"
    );
    let described = describe_when_active(&router, "an on-demand create", "on-demand-table").await;
    assert_eq!(
        described["BillingModeSummary"]["BillingMode"],
        json!("PAY_PER_REQUEST"),
        "case \"an on-demand create\": expected the stored mode read back, got {described}"
    );

    // An UPDATE that moves a table with no stored capacity into the provisioned mode and
    // names none is the same contradiction, which the update states in its own words.
    let (status, body) = call(
        &router,
        "UpdateTable",
        json!({"TableName": "on-demand-table", "BillingMode": "PROVISIONED"}),
    )
    .await;
    let said = message(&body);
    assert_eq!(
        (status, said.as_str()),
        (
            400,
            "One or more parameter values were invalid: ProvisionedThroughput must be specified \
             when BillingMode is PROVISIONED"
        ),
        "case \"a provisioned update with no capacity\": got {status} with {body}"
    );
    let unchanged = ok(
        &router,
        "DescribeTable",
        json!({"TableName": "on-demand-table"}),
    )
    .await;
    assert_eq!(
        unchanged["Table"]["BillingModeSummary"]["BillingMode"],
        json!("PAY_PER_REQUEST"),
        "case \"a provisioned update with no capacity\": the refused update must leave the mode \
         alone, got {unchanged}"
    );

    // The same rule one level down: an index of a provisioned table carries its own
    // capacity, and the sentence names the index it is missing from.
    describe_when_active(
        &router,
        "a provisioned create whose index states no capacity",
        "provisioned-table",
    )
    .await;
    for (case, operation, request) in [
        (
            "a provisioned create whose index states no capacity",
            "CreateTable",
            create_body(
                "indexed-capacity",
                json!({"BillingMode": "PROVISIONED",
                       "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                                 "WriteCapacityUnits": 1},
                       "GlobalSecondaryIndexes": [{
                           "IndexName": "by-pk",
                           "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                           "Projection": {"ProjectionType": "KEYS_ONLY"}}]}),
            ),
        ),
        (
            "a provisioned update whose new index states no capacity",
            "UpdateTable",
            json!({"TableName": "provisioned-table",
                   "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                   "GlobalSecondaryIndexUpdates": [{"Create": {
                       "IndexName": "by-pk",
                       "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                       "Projection": {"ProjectionType": "KEYS_ONLY"}}}]}),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        let said = message(&body);
        assert_eq!(
            (status, said.as_str()),
            (
                400,
                "One or more parameter values were invalid: Both ReadCapacityUnits and \
                 WriteCapacityUnits must be specified for index: by-pk"
            ),
            "case {case:?} ({operation}): got {status} with {body}"
        );
    }
    let switched = ok(
        &router,
        "UpdateTable",
        json!({"TableName": MODED, "BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    assert_eq!(
        switched["TableDescription"]["BillingModeSummary"]["BillingMode"],
        json!("PAY_PER_REQUEST"),
        "case \"an accepted switch\": expected the mode it asked for, got {switched}"
    );

    // The same two values through the OTHER member: a request states the mode through
    // either one, so a create and an update that use the throughput-mode member are served
    // and the table reports the mode they asked for under both names it answers it with.
    let moded = ok(
        &router,
        "CreateTable",
        create_body(
            "throughput-mode-table",
            json!({"TableThroughputMode": "PAY_PER_REQUEST"}),
        ),
    )
    .await;
    assert_eq!(
        (
            moded["TableDescription"]["BillingModeSummary"]["BillingMode"].clone(),
            moded["TableDescription"]["TableThroughputModeSummary"]["TableThroughputMode"]
                .clone(),
        ),
        (json!("PAY_PER_REQUEST"), json!("PAY_PER_REQUEST")),
        "case \"a create through the throughput-mode member\": expected the mode it asked for, \
         got {moded}"
    );
    describe_when_active(
        &router,
        "a create through the throughput-mode member",
        "throughput-mode-table",
    )
    .await;
    let moved = ok(
        &router,
        "UpdateTable",
        json!({"TableName": "throughput-mode-table",
               "TableThroughputMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": 4,
                                         "WriteCapacityUnits": 5}}),
    )
    .await;
    assert_eq!(
        moved["TableDescription"]["TableThroughputModeSummary"]["TableThroughputMode"],
        json!("PROVISIONED"),
        "case \"a switch through the throughput-mode member\": expected the mode it asked for, \
         got {moved}"
    );
    let settled = describe_when_active(
        &router,
        "a switch through the throughput-mode member",
        "throughput-mode-table",
    )
    .await;
    assert_eq!(
        (
            settled["BillingModeSummary"]["BillingMode"].clone(),
            settled["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            settled["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
        ),
        (json!("PROVISIONED"), json!(4), json!(5)),
        "case \"a switch through the throughput-mode member\": expected the stored mode and \
         capacity read back, got {settled}"
    );
}

// ===========================================================================
// RULE 3. A substitution table sent with no expression at all
// ===========================================================================

/// The table the expression rows address.
const SUBSTITUTED: &str = "Substituted";

/// The whole message a request that sent `member` and no expression earns, as an operation
/// whose expression members carry the `<N> validation error(s) detected: ` envelope states
/// it — the members a WRITE states its intent through.
fn only_with_expressions(member: &str) -> String {
    format!("1 validation error detected: {}", bare_sentence(member))
}

/// The same sentence where the envelope does not follow: an operation whose expression
/// members only NARROW a read, and one ELEMENT of a multi-item request, which this service
/// states every refusal about outside the request's own envelope.
fn only_with_expressions_bare(member: &str) -> String {
    bare_sentence(member)
}

/// The sentence itself, which is entirely fixed apart from the table it names.
fn bare_sentence(member: &str) -> String {
    format!("{member} can only be specified when using expressions")
}

#[tokio::test]
async fn a_substitution_table_with_no_expression_is_told_to_add_one() {
    let router = emulator();
    create_active_table(&router, SUBSTITUTED, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": SUBSTITUTED,
               "Item": {"pk": {"S": "item-1"}, "status": {"S": "active"}}}),
    )
    .await;
    let key = json!({"pk": {"S": "item-1"}});

    // (case, operation, request, the whole message it must earn)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "names, on a delete whose only expression member is absent",
            "DeleteItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ExpressionAttributeNames": {"#s": "status"}}),
            only_with_expressions("ExpressionAttributeNames"),
        ),
        (
            "values, on the same delete",
            "DeleteItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            only_with_expressions("ExpressionAttributeValues"),
        ),
        (
            "both tables at once: the names are reported",
            "DeleteItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ExpressionAttributeNames": {"#s": "status"},
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            only_with_expressions("ExpressionAttributeNames"),
        ),
        (
            "names, on a write",
            "PutItem",
            json!({"TableName": SUBSTITUTED,
                   "Item": {"pk": {"S": "item-2"}},
                   "ExpressionAttributeNames": {"#s": "status"}}),
            only_with_expressions("ExpressionAttributeNames"),
        ),
        (
            "values, on an update that names neither of its expression members",
            "UpdateItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ExpressionAttributeValues": {":v": {"S": "paused"}}}),
            only_with_expressions("ExpressionAttributeValues"),
        ),
        (
            "names, on a read with no projection",
            "GetItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ExpressionAttributeNames": {"#s": "status"}}),
            only_with_expressions_bare("ExpressionAttributeNames"),
        ),
        (
            "values, on a scan with no filter",
            "Scan",
            json!({"TableName": SUBSTITUTED, "ConsistentRead": true,
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            only_with_expressions_bare("ExpressionAttributeValues"),
        ),
        (
            "values, on a scan whose only expression is a projection",
            "Scan",
            json!({"TableName": SUBSTITUTED, "ConsistentRead": true,
                   "ProjectionExpression": "pk",
                   "ExpressionAttributeValues": {":v": {"S": "active"}}}),
            only_with_expressions_bare("ExpressionAttributeValues"),
        ),
        (
            "names, on one entry of a batched read",
            "BatchGetItem",
            json!({"RequestItems": {SUBSTITUTED: {
                "Keys": [key.clone()],
                "ExpressionAttributeNames": {"#s": "status"}}}}),
            only_with_expressions_bare("ExpressionAttributeNames"),
        ),
        (
            "names, on one element of a transactional read",
            "TransactGetItems",
            json!({"TransactItems": [{"Get": {
                "TableName": SUBSTITUTED, "Key": key.clone(),
                "ExpressionAttributeNames": {"#s": "status"}}}]}),
            only_with_expressions_bare("ExpressionAttributeNames"),
        ),
        (
            "values, on one element of a transactional write",
            "TransactWriteItems",
            json!({"TransactItems": [{"Put": {
                "TableName": SUBSTITUTED,
                "Item": {"pk": {"S": "item-3"}},
                "ExpressionAttributeValues": {":v": {"S": "paused"}}}}]}),
            only_with_expressions_bare("ExpressionAttributeValues"),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
             {body}"
        );
        let said = message(&body);
        assert_eq!(
            said, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {said:?}"
        );
    }

    // The BOUNDARY: the OTHER defect class keeps its own sentence. A request that DOES
    // name an expression and offers an entry it never references is told to drop the
    // entry, not to add an expression.
    for (case, operation, request, expected) in [
        (
            "an entry a delete's condition never references",
            "DeleteItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ConditionExpression": "attribute_exists(pk)",
                   "ExpressionAttributeNames": {"#s": "status"}}),
            "1 validation error detected: Value provided in ExpressionAttributeNames unused in \
             expressions: keys: {#s}"
                .to_string(),
        ),
        (
            "an entry a read's projection never references",
            "GetItem",
            json!({"TableName": SUBSTITUTED, "Key": key.clone(),
                   "ProjectionExpression": "pk",
                   "ExpressionAttributeNames": {"#s": "status"}}),
            "Value provided in ExpressionAttributeNames unused in expressions: keys: {#s}"
                .to_string(),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        let said = message(&body);
        assert_eq!(
            (status, said.as_str()),
            (400, expected.as_str()),
            "case {case:?} ({operation}): expected {expected:?}, got {status} with {body}"
        );
    }

    // The CONTROL: a request whose expression uses the tokens it defines is served, and
    // every refusal above left the store where it was.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": SUBSTITUTED, "Key": key.clone(),
               "ProjectionExpression": "#s",
               "ExpressionAttributeNames": {"#s": "status"}}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"status": {"S": "active"}}),
        "case \"a projection that uses its names\": expected the projected item, got {read}"
    );
    for absent in ["item-2", "item-3"] {
        let answered = ok(
            &router,
            "GetItem",
            json!({"TableName": SUBSTITUTED, "Key": {"pk": {"S": absent}}}),
        )
        .await;
        assert_eq!(
            answered.get("Item"),
            None,
            "case {absent:?}: a refused write must store nothing, got {answered}"
        );
    }
}

// ===========================================================================
// RULE 4. A SELECT clause that repeats one identifier
// ===========================================================================

/// The table the statement rows read.
const STATEMENTS: &str = "Statements";

#[tokio::test]
async fn a_select_clause_that_repeats_an_identifier_is_refused() {
    let router = emulator();
    create_active_table(&router, STATEMENTS, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": STATEMENTS,
               "Item": {"pk": {"S": "p"}, "s": {"S": "alpha"}, "n": {"N": "1"},
                        "l": {"L": [{"S": "first"}, {"NULL": true}, {"S": "second"}]},
                        "m": {"M": {"nested": {"S": "deep"}, "other": {"N": "2"}}},
                        "k": {"M": {"nested": {"S": "shallow"}}}}}),
    )
    .await;

    // (case, the SELECT clause, the identifier(s) the refusal must name)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "one attribute named twice",
            format!("SELECT s, s FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "s",
        ),
        (
            "an attribute and a nested path that reports under the same name",
            format!("SELECT nested, m.nested FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "nested",
        ),
        (
            "two nested paths whose last steps are the same name",
            format!("SELECT m.nested, k.nested FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "nested",
        ),
        (
            "two items aliased to one identifier",
            format!("SELECT s AS one, n AS one FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "one",
        ),
        (
            "an alias that collides with an attribute named as itself",
            format!("SELECT s, n AS s FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "s",
        ),
        (
            "two identifiers repeated, each named once in the order they repeat",
            format!("SELECT n, s, n, s FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            "n, s",
        ),
    ];

    for (case, statement, repeated) in rows {
        let (status, body) = call(&router, "ExecuteStatement", json!({"Statement": statement})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {body}"
        );
        let said = message(&body);
        let expected = format!("Duplicate identifiers in select clause: {repeated}");
        assert_eq!(
            said, expected,
            "case {case:?}: expected {expected:?}, got {said:?}"
        );
        assert!(
            body.get("Items").is_none(),
            "case {case:?}: a refused statement reports no items, got {body}"
        );
    }

    // The CONTROL: a list whose identifiers are distinct is served — including two paths
    // under one attribute whose last steps differ, and the same colliding pair with an
    // alias to tell them apart.
    for (case, statement, expected) in [
        (
            "two distinct attributes",
            format!("SELECT s, n FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            json!([{"s": {"S": "alpha"}, "n": {"N": "1"}}]),
        ),
        (
            "two paths under one attribute, reported under their own last steps",
            format!("SELECT m.nested, m.other FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            json!([{"nested": {"S": "deep"}, "other": {"N": "2"}}]),
        ),
        (
            // Two elements of one list are two entries of the answer: a projected path
            // is reported under the PATH, so the index tells them apart.
            "two indexed paths under one attribute, reported under their paths",
            format!("SELECT l[0], l[1] FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            json!([{"l[0]": {"S": "first"}, "l[1]": {"NULL": true}}]),
        ),
        (
            "the colliding pair, told apart by an alias",
            format!("SELECT m.nested, k.nested AS shallow FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            json!([{"nested": {"S": "deep"}, "shallow": {"S": "shallow"}}]),
        ),
        (
            "a list whose members include one of the NULL type",
            format!("SELECT l FROM \"{STATEMENTS}\" WHERE pk = 'p'"),
            json!([{"l": {"L": [{"S": "first"}, {"NULL": true}, {"S": "second"}]}}]),
        ),
    ] {
        let served = ok(&router, "ExecuteStatement", json!({"Statement": statement})).await;
        assert_eq!(
            served["Items"], expected,
            "case {case:?}: expected {expected}, got {served}"
        );
    }

    // A `SELECT *` names no identifier at all, so it is served whatever the item holds.
    let everything = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": format!("SELECT * FROM \"{STATEMENTS}\" WHERE pk = 'p'")}),
    )
    .await;
    assert_eq!(
        everything["Items"].as_array().map(Vec::len),
        Some(1),
        "case \"a star projection\": expected the one matching item, got {everything}"
    );
}
