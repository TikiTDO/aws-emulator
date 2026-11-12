// SPDX-License-Identifier: CC0-1.0

//! How the zero-ETL integration PROPERTY operations validate one request, end to end over
//! the wire through the shared [`crate::wire`] harness.
//!
//! Two rules, both of them about ORDER and about which member a refusal is allowed to blame:
//!
//! * a rule decided on the REQUEST BODY alone is answered BEFORE the request's `ResourceArn`
//!   is read, so a request that breaks a body rule is told about the body even when the ARN
//!   it carries would itself be refused — the create of an integration resource property
//!   stating both sides or neither, and the create of integration table properties stating
//!   both table configurations, an unsupported source member, or a target table name that is
//!   no identifier;
//! * `TargetTableConfig$TargetTableName` is the name the replicated table is created under on
//!   the integration's target, so it has to be a table identifier, while the `TableName` the
//!   request replicates names an entity on the SOURCE and is held to no such rule.
//!
//! The suites that cover the rest of each slice — the round trip, the record misses and the
//! reading of `ResourceArn` itself — are [`crate::integration_resource_properties`],
//! [`crate::integration_resource_property_delete_misses`] (the misses a delete answers at the
//! declared length bounds of that member) and [`crate::integration_table_properties`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The longest `TargetTableName` the model admits: "len 1..128" on `String128`
/// (`smithy_query shape=TargetTableConfig`, member `TargetTableName`).
const MAX_TARGET_TABLE_NAME: usize = 128;

/// The sentence a create that states both processing sides is refused with.
const BOTH_PROCESSING_SIDES: &str = "Both source processing properties and target processing \
     properties cannot be provided in the same request.";

/// The sentence a create that states neither processing side is refused with.
const NEITHER_PROCESSING_SIDE: &str = "Either source processing properties or target \
     processing properties must be provided in the request.";

/// The sentence a create that states both table configurations is refused with.
const BOTH_TABLE_CONFIGS: &str = "Source and Target config can't both be set.";

/// The sentence a `SourceTableConfig` member the service does not process yet is refused
/// with.
const UNSUPPORTED_SOURCE_MEMBERS: &str = "Unsupported source table properties are provided.";

/// The sentence a target table name carrying anything but identifier characters is refused
/// with.
const TARGET_TABLE_NAME_INVALID: &str = "Target Table Name contains invalid characters.";

/// The ARN of the Glue connection `name`, as this emulator mints a connection's ARN.
fn connection_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:connection/{name}")
}

/// The ARN of the Glue database `name`, the same way.
fn database_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/{name}")
}

/// Register a Glue database, so a row can state an ARN that resolves as well as one that
/// does not.
async fn create_database(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": name } }),
    )
    .await;
}

/// The `ResourceArn` values a row pairs its body with: each one is a value the operation
/// would refuse on its own, so a refusal naming the ARN would hide the body's own fault.
///
/// (case, the value, which member the ARN alone would be blamed on)
fn unreadable_resource_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            // The recorded case: the zero-ETL target lives in another service, so the ARN is
            // outside Glue's namespace.
            "a Redshift Serverless namespace ARN",
            format!("arn:aws:redshift-serverless:{REGION}:{ACCOUNT_ID}:namespace/warehouse"),
        ),
        ("a value that is no ARN at all", "warehouse".to_string()),
        (
            "a Glue ARN of a resource kind this family does not take",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:crawler/nightly"),
        ),
        (
            "a Glue connection this account does not hold",
            connection_arn("absent-connection"),
        ),
        (
            "a Glue database this account does not hold",
            database_arn("absent-database"),
        ),
    ]
}

/// A request that breaks a rule of the BODY is refused with that rule's sentence whatever its
/// `ResourceArn` carries: the body is read first, so the refusal names the payload the client
/// has to fix rather than a resource it never meant to address.
///
/// The rows differ along which body rule is broken and which operation reads it; each row is
/// then paired with every `ResourceArn` that would be refused on its own, including the ARN
/// that resolves to nothing at all.
#[tokio::test]
async fn a_body_rule_is_answered_before_the_resource_arn_is_read() {
    let router = emulator();

    // (case, the operation, the members beside `ResourceArn`, the sentence the refusal
    // carries)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "both processing sides at once",
            "CreateIntegrationResourceProperty",
            json!({
                "SourceProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/s" },
                "TargetProcessingProperties": { "RoleArn": "arn:aws:iam::000000000000:role/t" },
            }),
            BOTH_PROCESSING_SIDES,
        ),
        (
            "neither processing side",
            "CreateIntegrationResourceProperty",
            json!({ "Tags": [{ "key": "owner", "value": "data-platform" }] }),
            NEITHER_PROCESSING_SIDE,
        ),
        (
            "both table configurations at once",
            "CreateIntegrationTableProperties",
            json!({
                "TableName": "orders",
                "SourceTableConfig": { "PrimaryKey": ["id"] },
                "TargetTableConfig": { "TargetTableName": "orders_replica" },
            }),
            BOTH_TABLE_CONFIGS,
        ),
        (
            "a source table configuration the service does not process yet",
            "CreateIntegrationTableProperties",
            json!({
                "TableName": "orders",
                "SourceTableConfig": { "FilterPredicate": "created_at > 0" },
            }),
            UNSUPPORTED_SOURCE_MEMBERS,
        ),
        (
            // The recorded case: a target table name that is no identifier, sent against a
            // target ARN outside Glue's namespace.
            "a target table name that is no identifier",
            "CreateIntegrationTableProperties",
            json!({
                "TableName": "orders",
                "TargetTableConfig": { "TargetTableName": "orders-replica" },
            }),
            TARGET_TABLE_NAME_INVALID,
        ),
    ];

    for (case, operation, members, expected) in rows {
        for (arn_case, resource_arn) in unreadable_resource_arns() {
            let mut request = members.clone();
            request["ResourceArn"] = json!(resource_arn);
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case} with {arn_case}: expected 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case} with {arn_case}: expected ValidationException, got {}",
                error_code(&body)
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {case} with {arn_case}: expected the body rule's sentence, got {:?}",
                error_message(&body)
            );
        }
    }
}

/// `TargetTableConfig$TargetTableName` has to be a table identifier — ASCII letters, digits
/// and `_` — because it names the table the replication creates on the target. A name of any
/// other form is refused with the modelled `ValidationException` and nothing is stored for
/// it, while a name of that form is served and reads back.
///
/// The rows differ along the character class the name carries, its case, its length (both the
/// shortest and the longest the model admits) and, for the refused rows, which single
/// character makes the name no identifier.
#[tokio::test]
async fn a_target_table_name_must_be_a_table_identifier() {
    let router = emulator();
    create_database(&router, "identifiers").await;
    let resource_arn = database_arn("identifiers");

    let longest: String = "t".repeat(MAX_TARGET_TABLE_NAME);
    let accepted: Vec<(String, String)> = vec![
        ("lowercase letters".to_string(), "orders".to_string()),
        (
            "letters and underscores".to_string(),
            "orders_replica".to_string(),
        ),
        (
            "letters, digits and underscores".to_string(),
            "orders_2026_q1".to_string(),
        ),
        ("upper case letters".to_string(), "ORDERS".to_string()),
        ("mixed case".to_string(), "OrdersReplica".to_string()),
        ("digits alone".to_string(), "20260101".to_string()),
        ("an underscore alone".to_string(), "_".to_string()),
        (
            "a leading underscore".to_string(),
            "_staging_orders".to_string(),
        ),
        (
            "the shortest name the model admits".to_string(),
            "t".to_string(),
        ),
        (
            format!("the longest name the model admits ({MAX_TARGET_TABLE_NAME})"),
            longest,
        ),
    ];

    for (index, (case, target_table_name)) in accepted.iter().enumerate() {
        let table = format!("source/entity/{index}");
        let created = call(
            &router,
            "CreateIntegrationTableProperties",
            &json!({
                "ResourceArn": &resource_arn,
                "TableName": &table,
                "TargetTableConfig": { "TargetTableName": target_table_name },
            }),
        )
        .await;
        assert_eq!(
            created.0, 200,
            "case {case}: {target_table_name:?} is a table identifier and must be served, got {} \
             with {}",
            created.0, created.1
        );

        // The write is observable: the stored record reports the name it was given.
        let read = ok(
            &router,
            "GetIntegrationTableProperties",
            &json!({ "ResourceArn": &resource_arn, "TableName": &table }),
        )
        .await;
        assert_eq!(
            read.pointer("/TargetTableConfig/TargetTableName")
                .and_then(Value::as_str),
            Some(target_table_name.as_str()),
            "case {case}: expected the stored target table name, got {read}"
        );
    }

    // One row per character that is no identifier character, each inside an otherwise
    // well-formed name, plus the forms a client is most likely to send.
    let mut refused: Vec<(String, String)> = vec![
        // The recorded case: a generated name spelled with hyphens.
        (
            "a hyphenated name".to_string(),
            "orders-replica-2026".to_string(),
        ),
        ("a dotted name".to_string(), "sales.orders".to_string()),
        (
            "a source entity path".to_string(),
            "sap/entity/orders".to_string(),
        ),
        (
            "a name wrapped in spaces".to_string(),
            " orders ".to_string(),
        ),
        (
            "a name carrying a non-ASCII letter".to_string(),
            "orders_café".to_string(),
        ),
    ];
    for character in [
        '-', '.', '/', ' ', ':', '$', '#', '@', '%', '+', '*', '"', ',', ';', '!',
    ] {
        refused.push((
            format!("a name carrying {character:?}"),
            format!("orders{character}replica"),
        ));
    }

    for (index, (case, target_table_name)) in refused.iter().enumerate() {
        let table = format!("refused/entity/{index}");
        let (status, body) = call(
            &router,
            "CreateIntegrationTableProperties",
            &json!({
                "ResourceArn": &resource_arn,
                "TableName": &table,
                "TargetTableConfig": { "TargetTableName": target_table_name },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: {target_table_name:?} is no table identifier and must be refused, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case}: expected ValidationException for {target_table_name:?}, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            TARGET_TABLE_NAME_INVALID,
            "case {case}: expected the fixed refusal sentence, got {:?}",
            error_message(&body)
        );

        // The refusal stored nothing: the pair it named still has no record.
        let (read_status, read_body) = call(
            &router,
            "GetIntegrationTableProperties",
            &json!({ "ResourceArn": &resource_arn, "TableName": &table }),
        )
        .await;
        assert_eq!(
            read_status, 400,
            "case {case}: the refused create must have stored nothing, got {read_status} with \
             {read_body}"
        );
        assert_eq!(
            error_message(&read_body),
            format!(
                "Failed to find the sourceTableProperties with sourceArn {resource_arn} and \
                 tableName {table}"
            ),
            "case {case}: expected the miss of the pair the request named, got {read_body}"
        );
    }
}

/// The rule is on the TARGET table name alone: the `TableName` the request replicates names an
/// entity on the source, which spells its names its own way, so a value the target table name
/// could not take is served there and reads back unchanged.
///
/// The rows differ along the punctuation the source entity carries and whether the request
/// states a target configuration at all.
#[tokio::test]
async fn the_replicated_table_name_is_held_to_no_identifier_rule() {
    let router = emulator();
    create_database(&router, "entities").await;
    let resource_arn = database_arn("entities");

    for (case, table, config) in [
        (
            "a SAP entity path",
            "sap/entity/of/orders",
            json!({ "TargetTableConfig": { "TargetTableName": "orders_replica" } }),
        ),
        (
            "a hyphenated source table",
            "sales-orders-2026",
            json!({ "TargetTableConfig": { "TargetTableName": "sales_orders_2026" } }),
        ),
        (
            "a dotted source table, with no target configuration at all",
            "sales.orders",
            json!({ "SourceTableConfig": { "PrimaryKey": ["id"] } }),
        ),
        (
            "a source table carrying a space, with no configuration at all",
            "sales orders",
            json!({}),
        ),
        (
            "a source table carrying a non-ASCII letter",
            "ventes_café",
            json!({ "TargetTableConfig": {} }),
        ),
    ] {
        let mut request = config.clone();
        request["ResourceArn"] = json!(&resource_arn);
        request["TableName"] = json!(table);
        let (status, body) = call(&router, "CreateIntegrationTableProperties", &request).await;
        assert_eq!(
            status, 200,
            "case {case}: {table:?} is a source entity name and must be served, got {status} with \
             {body}"
        );

        let read = ok(
            &router,
            "GetIntegrationTableProperties",
            &json!({ "ResourceArn": &resource_arn, "TableName": table }),
        )
        .await;
        assert_eq!(
            read.get("TableName").and_then(Value::as_str),
            Some(table),
            "case {case}: the read must report the table it was asked for, got {read}"
        );
    }
}
