// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusals it states before it serves a request, over the
//! rules each of them follows rather than over one example of each.
//!
//! Six rules, each driven end to end over a table of inputs:
//!
//! 1. A required member this service's OWN request code demands — the table-name member,
//!    whose 3..=255 `[a-zA-Z0-9_.-]+` form the public model does not carry — is refused,
//!    when the request carries no value for it, with the service's parameter-required
//!    sentence: `The parameter '<Member>' is required but was not present in the
//!    request`, naming the member as the model spells it on the wire and nothing of the
//!    operation — except on the operations that are observed to state the validating
//!    walk's clause for that member too (`DeleteTable`). Every OTHER required member keeps
//!    that walk's clause inside the `<N> validation error(s) detected: ` envelope, naming
//!    the member by the path that walk reports it under, which is lowerCamel.
//! 2. When several members of one input fail, the clauses appear in the order the service
//!    states that input's members, which for the table listing is its page SIZE before
//!    its page CURSOR — the reverse of the order the model declares them in.
//! 3. A `RequestItems` map KEY that addresses no table is refused as a MAP KEY: the
//!    clause names the map member and lists the whole set a key must satisfy, rather than
//!    reporting a bound against a `tableName` member the request shape does not carry.
//! 4. A `BackupArn` that is no backup ARN of this endpoint is refused with AWS's fixed
//!    `Invalid Backup ARN`, whichever way the value failed and whichever operation read
//!    it, with nothing echoed.
//! 5. An ARN-addressed member that is NOT AN ARN AT ALL is refused by that rule — `One or
//!    more parameter values were invalid: ARNs must start with 'arn:': <value>` — while a
//!    well-formed ARN that addresses no table of this endpoint is refused by the member it
//!    was sent in, and an ARN of this service that names a resource of another KIND is
//!    refused by that type: `Invalid resource type in '<arn>'`.
//! 6. A reserved word is an attribute name only in an expression that PARSES: an
//!    expression the grammar cannot complete is the syntax error it is, reported at the
//!    token the parse stopped at, and only a complete expression earns the reserved-word
//!    refusal.
//!
//! Rule 5's lookup half is also covered: the resource-policy surface reports a missing
//! resource as the TABLE it is, by the name inside the ARN, while the tagging surface
//! names the `ResourceArn` member it was addressed by.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, call_in, emulator, error_code, message, ok, REGION};
use crate::table_items::create_active_table;

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The refusal `operation` answers `request` with: its code and its message, over a call
/// that must not succeed.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> (String, String) {
    let (status, answered) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?} ({operation}): expected HTTP 400, got {status} with {answered}"
    );
    (error_code(&answered), message(&answered))
}

// ===========================================================================
// 1. A required member that is absent
// ===========================================================================

#[tokio::test]
async fn an_absent_table_name_is_the_parameter_required_sentence_on_every_operation() {
    let router = emulator();

    // The rows span the planes and the shapes the member sits in: a create, a
    // description, an item read, an item write, a batch of writes, a tagging-adjacent
    // control-plane call and the Kinesis-destination surface. Every one of them declares
    // `TableName` required, so every one of them states the same sentence — the member
    // name is the only variable part, and it is the same member here.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a create with no table name",
            "CreateTable",
            json!({"KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                   "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                   "BillingMode": "PAY_PER_REQUEST"}),
        ),
        ("a description with no table name", "DescribeTable", json!({})),
        (
            "an item read with no table name",
            "GetItem",
            json!({"Key": {"pk": {"S": "one"}}}),
        ),
        (
            "an item write with no table name",
            "PutItem",
            json!({"Item": {"pk": {"S": "one"}}}),
        ),
        (
            "an item delete with no table name",
            "DeleteItem",
            json!({"Key": {"pk": {"S": "one"}}}),
        ),
        (
            "a query with no table name",
            "Query",
            json!({"KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
        ),
        ("a scan with no table name", "Scan", json!({})),
        (
            "a backup with no table name",
            "CreateBackup",
            json!({"BackupName": "nightly"}),
        ),
        (
            "a time-to-live read with no table name",
            "DescribeTimeToLive",
            json!({}),
        ),
        (
            "a Kinesis-destination read with no table name",
            "DescribeKinesisStreamingDestination",
            json!({}),
        ),
        (
            "a Kinesis-destination enable with no table name",
            "EnableKinesisStreamingDestination",
            json!({"StreamArn": format!(
                "arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/changes"
            )}),
        ),
    ];

    let expected = "The parameter 'TableName' is required but was not present in the request";
    for (case, operation, request) in rows {
        let (code, stated) = refusal(&router, case, operation, request).await;
        assert_eq!(
            (code.as_str(), stated.as_str()),
            ("ValidationException", expected),
            "case {case:?} ({operation}): expected the parameter-required sentence, got \
             {code} with {stated:?}"
        );
    }

    // The sentence is the WHOLE message, so a request that omits the member AND breaks
    // another member's bound still states it alone: the check that finds the parameter
    // absent is not the validating walk, and it answers before any clause is built. The
    // rows break a bound of a different class each time — an enum, a `@range` and another
    // required member — so the precedence is not pinned to one companion fault.
    for (case, operation, request) in [
        (
            "no table name and a value outside an enum's set",
            "GetItem",
            json!({"Key": {"pk": {"S": "one"}}, "ReturnConsumedCapacity": "NOPE"}),
        ),
        (
            "no table name and a page size below its minimum",
            "Query",
            json!({"Limit": 0, "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
        ),
        (
            "no table name and no key either",
            "GetItem",
            json!({"ReturnConsumedCapacity": "TOTAL"}),
        ),
    ] {
        let (code, stated) = refusal(&router, case, operation, request).await;
        assert_eq!(
            (code.as_str(), stated.as_str()),
            ("ValidationException", expected),
            "case {case:?} ({operation}): expected the parameter-required sentence alone, got \
             {code} with {stated:?}"
        );
    }
}

#[tokio::test]
async fn every_other_absent_required_member_keeps_the_walks_clause() {
    let router = emulator();
    create_active_table(&router, "Members", &[("pk", "HASH", "S")]).await;
    let table_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/Members");

    // (case, operation, request, the member path the clause names). The rows span the
    // member's own shape — a string, an ARN, a map, a list, a structure — and both
    // planes, and each request carries every OTHER required member so the absent one is
    // the only fault. The path is the lowerCamel one the validating walk reports, which
    // is what a client reads out of the message.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a stream destination with no stream ARN",
            "EnableKinesisStreamingDestination",
            json!({"TableName": "Members"}),
            "streamArn",
        ),
        (
            "a destination disable with no stream ARN",
            "DisableKinesisStreamingDestination",
            json!({"TableName": "Members"}),
            "streamArn",
        ),
        (
            "an item read with no key",
            "GetItem",
            json!({"TableName": "Members"}),
            "key",
        ),
        (
            "an item write with no item",
            "PutItem",
            json!({"TableName": "Members"}),
            "item",
        ),
        (
            "an item delete with no key",
            "DeleteItem",
            json!({"TableName": "Members"}),
            "key",
        ),
        (
            "a batch read with no request items",
            "BatchGetItem",
            json!({}),
            "requestItems",
        ),
        (
            "a backup description with no backup ARN",
            "DescribeBackup",
            json!({}),
            "backupArn",
        ),
        (
            "a tag listing with no resource ARN",
            "ListTagsOfResource",
            json!({}),
            "resourceArn",
        ),
        (
            "a tag write with no tags",
            "TagResource",
            json!({"ResourceArn": table_arn.clone()}),
            "tags",
        ),
        (
            "a tag removal with no tag keys",
            "UntagResource",
            json!({"ResourceArn": table_arn.clone()}),
            "tagKeys",
        ),
        (
            "a policy attach with no document",
            "PutResourcePolicy",
            json!({"ResourceArn": table_arn.clone()}),
            "policy",
        ),
        (
            "a time-to-live write with no specification",
            "UpdateTimeToLive",
            json!({"TableName": "Members"}),
            "timeToLiveSpecification",
        ),
        (
            "a backup with no backup name",
            "CreateBackup",
            json!({"TableName": "Members"}),
            "backupName",
        ),
        // The table-name member itself on the one operation observed to state the walk's
        // clause for it, which is what tells the two groups of rule 1 apart: the same
        // absence of the same member, stated as a clause here and as the service's own
        // sentence by every operation above.
        (
            "a table delete with no table name",
            "DeleteTable",
            json!({}),
            "tableName",
        ),
    ];

    for (case, operation, request, path) in rows {
        let (code, stated) = refusal(&router, case, operation, request).await;
        let expected = format!(
            "1 validation error detected: Value null at '{path}' failed to satisfy constraint: \
             Member must not be null"
        );
        assert_eq!(
            (code.as_str(), stated.as_str()),
            ("ValidationException", expected.as_str()),
            "case {case:?} ({operation}): expected the walk's clause over {path:?}, got \
             {code} with {stated:?}"
        );
    }
}

// ===========================================================================
// 2. The order several clauses are stated in
// ===========================================================================

#[tokio::test]
async fn a_table_listing_states_its_page_size_clause_before_its_cursor_clause() {
    let router = emulator();

    // The rows differ along which bound each member broke — the page size below its
    // minimum and above its maximum, the cursor below its minimum length and outside its
    // pattern — so the ORDER is asserted over four different clause pairs rather than
    // over one.
    let long_name = "n".repeat(256);
    let rows: Vec<(&str, Value, String, String)> = vec![
        (
            "a page size below its minimum and a cursor below its minimum length",
            json!({"Limit": 0, "ExclusiveStartTableName": "ab"}),
            "Value '0' at 'limit' failed to satisfy constraint: Member must have value greater \
             than or equal to 1"
                .to_string(),
            "Value 'ab' at 'exclusiveStartTableName' failed to satisfy constraint: Member must \
             have length greater than or equal to 3"
                .to_string(),
        ),
        (
            "a page size above its maximum and a cursor past its maximum length",
            json!({"Limit": 101, "ExclusiveStartTableName": long_name.clone()}),
            "Value '101' at 'limit' failed to satisfy constraint: Member must have value less \
             than or equal to 100"
                .to_string(),
            format!(
                "Value '{long_name}' at 'exclusiveStartTableName' failed to satisfy constraint: \
                 Member must have length less than or equal to 255"
            ),
        ),
        (
            "a page size below its minimum and a cursor outside its pattern",
            json!({"Limit": -1, "ExclusiveStartTableName": "not a name"}),
            "Value '-1' at 'limit' failed to satisfy constraint: Member must have value greater \
             than or equal to 1"
                .to_string(),
            "Value 'not a name' at 'exclusiveStartTableName' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
    ];

    for (case, request, size_clause, cursor_clause) in rows {
        let (code, stated) = refusal(&router, case, "ListTables", request).await;
        let expected = format!("2 validation errors detected: {size_clause}; {cursor_clause}");
        assert_eq!(
            (code.as_str(), stated.as_str()),
            ("ValidationException", expected.as_str()),
            "case {case:?}: expected the page-size clause first, got {code} with {stated:?}"
        );
    }

    // One member alone is one clause, so the order above is about which comes first and
    // not about which clauses are stated at all.
    for (case, request, expected) in [
        (
            "the page size alone",
            json!({"Limit": 0}),
            "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: \
             Member must have value greater than or equal to 1",
        ),
        (
            "the cursor alone",
            json!({"ExclusiveStartTableName": "ab"}),
            "1 validation error detected: Value 'ab' at 'exclusiveStartTableName' failed to \
             satisfy constraint: Member must have length greater than or equal to 3",
        ),
    ] {
        let (code, stated) = refusal(&router, case, "ListTables", request).await;
        assert_eq!(
            (code.as_str(), stated.as_str()),
            ("ValidationException", expected),
            "case {case:?}: expected the single clause, got {code} with {stated:?}"
        );
    }

    // A listing inside both bounds is served, so the refusals above are about the values
    // sent and not about the operation.
    let listed = ok(&router, "ListTables", json!({"Limit": 1})).await;
    assert!(
        listed["TableNames"].is_array(),
        "case a listing inside both bounds: expected a page of names, got {listed}"
    );
}

// ===========================================================================
// 3. A RequestItems map key that addresses no table
// ===========================================================================

/// The clause a refused `RequestItems` key earns: the map member, and the whole set a key
/// must satisfy — the table-name form, which the model does not carry on the key shape
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#BatchGetRequestMap")`
/// gives a key of `TableArn`, length 1..1024).
///
/// `quoted` is the map the clause states as the value it refused, which `BatchGetItem`
/// renders as the JSON the request carried and `BatchWriteItem` as the request
/// validator's own object form (`request_shape_validation_classes` spans that rule).
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
async fn a_request_items_key_that_addresses_no_table_is_refused_as_a_map_key() {
    let router = emulator();
    create_active_table(&router, "Batched", &[("pk", "HASH", "S")]).await;

    // The rows span the ways a key can fail to name a table: too short, too long, a
    // character the name pattern does not admit, an ARN of another service and a
    // sub-resource ARN of this one.
    let long_key = "k".repeat(256);
    let keys: Vec<(&str, String)> = vec![
        ("a key below the minimum name length", "ab".to_string()),
        ("a key past the maximum name length", long_key),
        ("a key outside the name pattern", "bad name!".to_string()),
        (
            "a key that is an ARN of another service",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:Batched"),
        ),
        (
            "a key that is an index sub-resource ARN",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/Batched/index/by-pk"),
        ),
    ];

    for (case, key) in &keys {
        let quoted = format!(
            "{{{}:{{\"Keys\":[{{\"pk\":{{\"S\":\"one\"}}}}]}}}}",
            serde_json::Value::String(key.clone())
        );
        let (code, stated) = refusal(
            &router,
            case,
            "BatchGetItem",
            json!({"RequestItems": {key: {"Keys": [{"pk": {"S": "one"}}]}}}),
        )
        .await;
        assert_eq!(
            (code.as_str(), stated.as_str()),
            ("ValidationException", map_key_clause(Some(&quoted)).as_str()),
            "case {case:?} (BatchGetItem): expected the map-key clause, got {code} with \
             {stated:?}"
        );
        assert!(
            !stated.contains("tableName"),
            "case {case:?}: a map key is no member of the request shape, got {stated:?}"
        );

        let (code, stated) = refusal(
            &router,
            case,
            "BatchWriteItem",
            json!({"RequestItems": {key: [{"PutRequest": {"Item": {"pk": {"S": "one"}}}}]}}),
        )
        .await;
        let written = format!(
            "{{{key}=[WriteRequest(putRequest=PutRequest(item={{pk=AttributeValue(s=one, \
             n=null, b=null, iNT=null, dECIMAL=null, dOUBLE=null, sS=null, nS=null, bS=null, \
             iNTSET=null, dECIMALSET=null, dOUBLESET=null, m=null, l=null, nULL=null, \
             bOOL=null)}}, workloadProfileName=null), deleteRequest=null)]}}"
        );
        assert_eq!(
            (code.as_str(), stated.as_str()),
            (
                "ValidationException",
                map_key_clause(Some(&written)).as_str()
            ),
            "case {case:?} (BatchWriteItem): expected the map-key clause, got {code} with \
             {stated:?}"
        );
    }

    // Both spellings the key accepts still address the table: "each table name or ARN can
    // be used only once per BatchGetItem request" (`BatchGetItem$RequestItems`), so the
    // refusals above are about keys that name no table at all.
    for (case, key) in [
        ("the bare table name", "Batched".to_string()),
        (
            "the table's own ARN",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/Batched"),
        ),
    ] {
        let served = ok(
            &router,
            "BatchGetItem",
            json!({"RequestItems": {key.clone(): {"Keys": [{"pk": {"S": "one"}}]}}}),
        )
        .await;
        assert!(
            served["Responses"].is_object(),
            "case {case:?}: expected a served batch, got {served}"
        );
    }
}

// ===========================================================================
// 4. A BackupArn that is no backup ARN
// ===========================================================================

#[tokio::test]
async fn a_backup_arn_this_endpoint_cannot_address_is_the_fixed_refusal() {
    let router = emulator();
    create_active_table(&router, "Backed", &[("pk", "HASH", "S")]).await;
    let created = ok(
        &router,
        "CreateBackup",
        json!({"TableName": "Backed", "BackupName": "nightly"}),
    )
    .await;
    let real = created["BackupDetails"]["BackupArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let table_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/Backed");

    // The rows span the ways a value can fail to be a backup ARN of this endpoint: not an
    // ARN, an ARN of another service, the TABLE's ARN rather than a backup's, an empty
    // backup segment, a deeper segment, and another partition, account or region.
    let values: Vec<(&str, String)> = vec![
        (
            "a value that is no ARN at all",
            "not-an-arn-but-long-enough-to-be-modelled".to_string(),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:table/Backed/backup/01700000000000-abcdef12"),
        ),
        ("the table's own ARN", table_arn.clone()),
        ("an empty backup segment", format!("{table_arn}/backup/")),
        (
            "a segment deeper than a backup",
            format!("{table_arn}/backup/01700000000000-abcdef12/part/1"),
        ),
        (
            "another partition",
            real.replace("arn:aws:", "arn:aws-cn:"),
        ),
        (
            "another account",
            real.replace(ACCOUNT, "111111111111"),
        ),
        (
            "another region",
            real.replace(&format!(":{REGION}:"), ":eu-west-1:"),
        ),
    ];

    for (case, value) in &values {
        for operation in ["DescribeBackup", "DeleteBackup"] {
            let (code, stated) = refusal(
                &router,
                case,
                operation,
                json!({"BackupArn": value}),
            )
            .await;
            assert_eq!(
                (code.as_str(), stated.as_str()),
                ("ValidationException", "Invalid Backup ARN"),
                "case {case:?} ({operation}): expected the fixed refusal, got {code} with \
                 {stated:?}"
            );
            assert!(
                !stated.contains(value.as_str()),
                "case {case:?} ({operation}): the refusal echoes nothing, got {stated:?}"
            );
        }
        // The restore path reads the same member through the same check.
        let (code, stated) = refusal(
            &router,
            case,
            "RestoreTableFromBackup",
            json!({"BackupArn": value, "TargetTableName": "Restored"}),
        )
        .await;
        assert_eq!(
            (code.as_str(), stated.as_str()),
            ("ValidationException", "Invalid Backup ARN"),
            "case {case:?} (RestoreTableFromBackup): expected the fixed refusal, got {code} \
             with {stated:?}"
        );
    }

    // The backup that IS addressable is still described, so the refusals above are about
    // the values sent.
    let described = ok(&router, "DescribeBackup", json!({"BackupArn": real.clone()})).await;
    assert_eq!(
        described["BackupDescription"]["BackupDetails"]["BackupArn"],
        json!(real),
        "case the backup's own ARN: expected the catalogued backup, got {described}"
    );
}

// ===========================================================================
// 5. An ARN-addressed member that is not an ARN, and a table ARN of another kind
// ===========================================================================

#[tokio::test]
async fn a_table_arn_naming_another_kind_of_resource_is_refused_by_its_type() {
    let router = emulator();
    create_active_table(&router, "Typed", &[("pk", "HASH", "S")]).await;

    // The rows span the resource types an ARN of this service can name where a table's
    // belongs — a backup, a mis-cased `Table`, an import and no type at all — over
    // operations on both planes. The type is what is wrong with every one of them, so
    // every one states the same sentence over the ARN that was sent.
    let arns: Vec<(&str, String)> = vec![
        (
            "a backup ARN",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:backup/Typed"),
        ),
        (
            "a mis-cased table type",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:Table/Typed"),
        ),
        (
            "an import ARN",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:import/01700000000000-abcdef12"),
        ),
        (
            "an ARN with no resource type",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:Typed"),
        ),
    ];

    for (case, arn) in &arns {
        let stream = format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/cdc");
        for (operation, request) in [
            ("DescribeTable", json!({"TableName": arn})),
            (
                "DescribeKinesisStreamingDestination",
                json!({"TableName": arn}),
            ),
            (
                "EnableKinesisStreamingDestination",
                json!({"TableName": arn, "StreamArn": stream}),
            ),
            (
                "GetItem",
                json!({"TableName": arn, "Key": {"pk": {"S": "one"}}}),
            ),
        ] {
            let (code, stated) = refusal(&router, case, operation, request).await;
            let expected =
                format!("1 validation error detected: Invalid resource type in '{arn}'");
            assert_eq!(
                (code.as_str(), stated.as_str()),
                ("ValidationException", expected.as_str()),
                "case {case:?} ({operation}): expected the resource-type refusal, got {code} \
                 with {stated:?}"
            );
        }
    }

    // The table's OWN ARN still names it, so the refusals above are about the type and
    // not about ARNs in the member: "You can also provide the Amazon Resource Name (ARN)
    // of the table in this parameter" (`DescribeTable$TableName`).
    let table_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/Typed");
    let described = ok(&router, "DescribeTable", json!({"TableName": table_arn})).await;
    assert_eq!(
        described["Table"]["TableName"],
        json!("Typed"),
        "case the table's own ARN: expected the table it names, got {described}"
    );
}

/// Every operation whose whole selector is a `ResourceArn`, with a body that is otherwise
/// valid.
fn arn_addressed(resource_arn: &str) -> Vec<(&'static str, Value)> {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Deny", "Principal": {"AWS": "*"},
                       "Action": "dynamodb:GetItem", "Resource": resource_arn}],
    })
    .to_string();
    vec![
        (
            "TagResource",
            json!({"ResourceArn": resource_arn, "Tags": [{"Key": "env", "Value": "prod"}]}),
        ),
        (
            "UntagResource",
            json!({"ResourceArn": resource_arn, "TagKeys": ["env"]}),
        ),
        ("ListTagsOfResource", json!({"ResourceArn": resource_arn})),
        (
            "PutResourcePolicy",
            json!({"ResourceArn": resource_arn, "Policy": policy}),
        ),
        ("GetResourcePolicy", json!({"ResourceArn": resource_arn})),
        ("DeleteResourcePolicy", json!({"ResourceArn": resource_arn})),
    ]
}

#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_is_refused_by_that_rule() {
    let router = emulator();
    create_active_table(&router, "Addressed", &[("pk", "HASH", "S")]).await;

    // The rows span the values that are not ARNs: a bare table name (the one a caller
    // sends by mistake), a name of no table at all, something ARN-shaped without the
    // scheme, and a single character.
    let not_arns = [
        ("a bare table name", "Addressed"),
        ("a bare name of no table", "absent-table"),
        (
            "an ARN with its scheme missing",
            "aws:dynamodb:us-east-1:000000000000:table/Addressed",
        ),
        ("a single character", "x"),
    ];
    for (case, value) in not_arns {
        for (operation, body) in arn_addressed(value) {
            let (code, stated) = refusal(&router, case, operation, body).await;
            assert_eq!(
                (code.as_str(), stated.as_str()),
                (
                    "ValidationException",
                    format!("One or more parameter values were invalid: ARNs must start with \
                             'arn:': {value}")
                        .as_str()
                ),
                "case {case:?} ({operation}): expected the ARN-prefix refusal, got {code} with \
                 {stated:?}"
            );
        }
    }

    // A value that IS an ARN and still addresses no resource of this endpoint is refused
    // by the member it was sent in: the rule it broke is which resource it names. Each
    // surface words that over the resources IT addresses — a table for the tagging
    // surface, "a table or stream" (the `PutResourcePolicy` doc) for the policy surface —
    // so the two sentences are asserted side by side.
    let arn_shaped = [
        (
            "an ARN of another service",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:Addressed"),
            format!(
                "One or more parameter values were invalid: Invalid resource arn provided, only \
                 table or stream is accepted. Provided resource arn: arn:aws:sqs:{REGION}:\
                 {ACCOUNT}:Addressed"
            ),
        ),
        (
            "an ARN of another region",
            format!("arn:aws:dynamodb:eu-west-1:{ACCOUNT}:table/Addressed"),
            format!(
                "One or more parameter values were invalid: Invalid resource Arn: \
                 arn:aws:dynamodb:eu-west-1:{ACCOUNT}:table/Addressed"
            ),
        ),
    ];
    for (case, value, policy_refusal) in &arn_shaped {
        for (operation, body) in arn_addressed(value) {
            let (code, stated) = refusal(&router, case, operation, body).await;
            let expected = if operation.ends_with("ResourcePolicy") {
                policy_refusal.clone()
            } else {
                format!("Invalid TableArn: Invalid ResourceArn provided as input {value}")
            };
            assert_eq!(
                (code.as_str(), stated.as_str()),
                ("ValidationException", expected.as_str()),
                "case {case:?} ({operation}): expected the member refusal, got {code} with \
                 {stated:?}"
            );
        }
    }

    // A STREAM sub-resource ARN is not a resource the tagging surface addresses at all,
    // while the policy surface takes it and looks the stream up — which is where a table
    // publishing to no stream answers that the stream is not there.
    let stream_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/Addressed/stream/2026-01-01");
    for (operation, body) in arn_addressed(&stream_arn) {
        let (code, stated) = refusal(&router, "a stream sub-resource ARN", operation, body).await;
        if operation == "PutResourcePolicy" {
            // This body's document names a TABLE action, which a stream's policy cannot
            // carry: the document is read before the resource is looked up, so that is
            // the refusal it earns.
            assert_eq!(
                (code.as_str(), stated.as_str()),
                (
                    "ValidationException",
                    "One or more parameter values were invalid: Invalid policy document: Policy \
                     has an action that is not supported for the resource: dynamodb:GetItem"
                ),
                "case a stream sub-resource ARN ({operation}): expected the unsupported-action \
                 refusal, got {code} with {stated:?}"
            );
        } else if operation.ends_with("ResourcePolicy") {
            assert_eq!(
                (code.as_str(), stated.as_str()),
                (
                    "ResourceNotFoundException",
                    format!("Requested resource not found: Stream: {stream_arn} not found")
                        .as_str()
                ),
                "case a stream sub-resource ARN ({operation}): expected the stream not-found, \
                 got {code} with {stated:?}"
            );
        } else {
            assert_eq!(
                (code.as_str(), stated.as_str()),
                (
                    "ValidationException",
                    format!(
                        "Invalid TableArn: Invalid ResourceArn provided as input {stream_arn}"
                    )
                    .as_str()
                ),
                "case a stream sub-resource ARN ({operation}): expected the member refusal, got \
                 {code} with {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_policy_surface_names_the_missing_table_and_the_tagging_surface_the_member() {
    let router = emulator();
    create_active_table(&router, "Present", &[("pk", "HASH", "S")]).await;

    // The rows differ along the name inside an otherwise addressable ARN, at the
    // documented minimum name length, with each character class the pattern allows, and
    // at the documented maximum.
    let absent: Vec<String> = vec![
        "abc".to_string(),
        "absent-table".to_string(),
        "dot.ted.name".to_string(),
        "under_scored".to_string(),
        "MixedCase99".to_string(),
        "z".repeat(255),
    ];
    for name in &absent {
        let resource_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/{name}");
        for (operation, body) in arn_addressed(&resource_arn) {
            let (status, answered) = call_in(&router, REGION, operation, body).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ResourceNotFoundException"),
                "case {name:?} on {operation}: expected 400 ResourceNotFoundException, got \
                 {status} with {answered}"
            );
            let stated = message(&answered);
            // The policy surface reports the resource as the TABLE it is; the tagging
            // surface names the member it was addressed by.
            let expected = if operation.ends_with("ResourcePolicy") {
                format!("Requested resource not found: Table: {name} not found")
            } else {
                format!("Requested resource not found: ResourceArn: {resource_arn} not found")
            };
            assert_eq!(
                stated, expected,
                "case {name:?} on {operation}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    // A table that IS there answers the policy read rather than a not-found, so the
    // refusals above are about the resource and not about the surface.
    let present_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/Present");
    let (status, answered) = call(
        &router,
        "GetResourcePolicy",
        json!({"ResourceArn": present_arn}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "PolicyNotFoundException"),
        "case a table with no policy: expected the policy refusal, got {status} with {answered}"
    );
}

// ===========================================================================
// 6. A reserved word is an attribute name only in an expression that parses
// ===========================================================================

#[tokio::test]
async fn an_expression_that_cannot_parse_is_the_syntax_error_it_is() {
    let router = emulator();
    create_active_table(&router, "Conditions", &[("pk", "HASH", "S")]).await;

    // (case, the ConditionExpression, the token the parse stopped at, the text around
    // it). The rows span reserved and ordinary words in the same incomplete position —
    // the whole point of the rule — and both ends of the published reserved-word list,
    // plus an expression that stops mid-way.
    let incomplete: Vec<(&str, &str, &str, &str)> = vec![
        ("a bare reserved word", "status", "<EOF>", "status"),
        (
            "a reserved word from the start of the list",
            "ABORT",
            "<EOF>",
            "ABORT",
        ),
        (
            "a reserved word from the end of the list",
            "zone",
            "<EOF>",
            "zone",
        ),
        (
            "a bare ordinary word, which cannot parse either",
            "pk",
            "<EOF>",
            "pk",
        ),
        (
            "a comparison that stops before its operand",
            "status =",
            "<EOF>",
            "=",
        ),
    ];

    for (case, expression, token, near) in incomplete {
        for operation in ["DeleteItem", "PutItem"] {
            let request = if operation == "PutItem" {
                json!({"TableName": "Conditions", "Item": {"pk": {"S": "one"}},
                       "ConditionExpression": expression})
            } else {
                json!({"TableName": "Conditions", "Key": {"pk": {"S": "one"}},
                       "ConditionExpression": expression})
            };
            let (code, stated) = refusal(&router, case, operation, request).await;
            let expected = format!(
                "1 validation error detected: Invalid ConditionExpression: Syntax error; \
                 token: \"{token}\", near: \"{near}\""
            );
            assert_eq!(
                (code.as_str(), stated.as_str()),
                ("ValidationException", expected.as_str()),
                "case {case:?} ({operation}): expected the syntax error, got {code} with \
                 {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_reserved_word_in_an_expression_that_parses_is_still_the_reserved_word_refusal() {
    let router = emulator();
    create_active_table(&router, "Reserved", &[("pk", "HASH", "S")]).await;

    // (case, operation, request, the member, the word). Every expression here PARSES, so
    // the name position is a name position and the reserved word is what is wrong with
    // it. The rows span the members that carry an expression and the positions a name
    // takes inside one: a function argument, an update target, a nested path step and a
    // comparison operand.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a reserved word as a function argument",
            "DeleteItem",
            json!({"TableName": "Reserved", "Key": {"pk": {"S": "one"}},
                   "ConditionExpression": "attribute_exists(status)"}),
            "ConditionExpression",
            "status",
        ),
        (
            "a reserved word as a comparison operand",
            "DeleteItem",
            json!({"TableName": "Reserved", "Key": {"pk": {"S": "one"}},
                   "ConditionExpression": "zone = :z",
                   "ExpressionAttributeValues": {":z": {"S": "a"}}}),
            "ConditionExpression",
            "zone",
        ),
        (
            "a reserved word as an update target",
            "UpdateItem",
            json!({"TableName": "Reserved", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET comment = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "UpdateExpression",
            "comment",
        ),
        (
            "a reserved word inside a document path",
            "UpdateItem",
            json!({"TableName": "Reserved", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET m.timestamp = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "UpdateExpression",
            "timestamp",
        ),
    ];

    for (case, operation, request, member, word) in rows {
        let (code, stated) = refusal(&router, case, operation, request).await;
        let expected = format!(
            "1 validation error detected: Invalid {member}: Attribute name is a reserved \
             keyword; reserved keyword: {word}"
        );
        assert_eq!(
            (code.as_str(), stated.as_str()),
            ("ValidationException", expected.as_str()),
            "case {case:?} ({operation}): expected the reserved-word refusal, got {code} with \
             {stated:?}"
        );
    }

    // The same name reached through `ExpressionAttributeNames` is served — which is what
    // that member is for — so the refusals above are about the bare word.
    let served = ok(
        &router,
        "PutItem",
        json!({"TableName": "Reserved", "Item": {"pk": {"S": "one"}, "status": {"S": "live"}},
               "ConditionExpression": "attribute_not_exists(#s)",
               "ExpressionAttributeNames": {"#s": "status"}}),
    )
    .await;
    assert!(
        served.is_object(),
        "case an aliased reserved word: expected the write to be served, got {served}"
    );
}
