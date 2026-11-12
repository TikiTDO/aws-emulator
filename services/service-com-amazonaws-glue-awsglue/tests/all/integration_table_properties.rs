// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue integration table property slice:
//! `CreateIntegrationTableProperties`, the `GetIntegrationTableProperties` that reads back
//! what it stored and the `DeleteIntegrationTableProperties` that removes it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rules under test:
//!
//! * a record is addressed by the `ResourceArn` naming one leg of the integration AND the
//!   `TableName` it overrides, and there is ONE record per pair — so a create for a pair
//!   that already has one writes that record, each stated side replacing the stored one and
//!   each unstated side left alone;
//! * a read reaches a record only through the ARN of an integration LEG (the Glue
//!   connection of the source, the Glue database of the target): a create against a
//!   replicated table's own ARN is accepted and that record is not readable through it;
//! * a delete resolves the record the same way and removes that ONE record, so a later read
//!   of the pair misses and every other pair is left as stored;
//! * a read or a delete that reaches no record names the PAIR it was asked for, whatever the
//!   resource carries;
//! * a `ResourceArn` none of the three can interpret is refused with the modelled
//!   `ValidationException` naming that value;
//! * one call carries the configuration of the one leg its `ResourceArn` names, so a call
//!   stating both sides is refused;
//! * a `SourceTableConfig` member the service does not process yet is refused, and so is a
//!   `TargetTableName` that is no table identifier.
//!
//! The round-trip suite is a table whose rows differ along which side of the integration
//! the configuration belongs to, which resource kind the ARN names, how many members the
//! side carries, the published value each enumerated member takes, the length of its
//! strings and the number of partitions it lists.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateIntegrationTablePropertiesInput;
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, IntegrationTablePropertiesEntry,
    MAX_INTEGRATION_TABLE_PROPERTIES, UNSUPPORTED_SOURCE_TABLE_CONFIG_MEMBERS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    assert_echoed, call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID, REGION,
};

/// The longest `TableName`, `TargetTableName` or `FilterPredicate` the model admits: "len
/// 1..128" on `String128` (`smithy_query operation=CreateIntegrationTableProperties`,
/// `shape=SourceTableConfig,TargetTableConfig`).
const MAX_NAME_LENGTH: usize = 128;

/// Every published `UnnestSpec` value: "Valid values are: \"TOPLEVEL\", \"FULL\", or
/// \"NOUNNEST\"" (`smithy_query shape=UnnestSpec`).
const UNNEST_SPECS: [&str; 3] = ["TOPLEVEL", "FULL", "NOUNNEST"];

/// Every published `FunctionSpec` of a partition: `identity`, `year`, `month`, `day` and
/// `hour` (`smithy_query shape=IntegrationPartition`, member `FunctionSpec`).
const FUNCTION_SPECS: [&str; 5] = ["identity", "year", "month", "day", "hour"];

/// Every published `ConversionSpec`: `epoch_sec`, `epoch_milli` and `iso` (same shape,
/// member `ConversionSpec`).
const CONVERSION_SPECS: [&str; 3] = ["epoch_sec", "epoch_milli", "iso"];

/// The ARN of the Glue connection `name`, as this emulator mints a connection's ARN.
fn connection_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:connection/{name}")
}

/// The ARN of the Glue database `name`, the same way.
fn database_arn(name: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/{name}")
}

/// The ARN of the catalog table `table` of `database`, whose resource portion carries both
/// names.
fn table_arn(database: &str, table: &str) -> String {
    format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:table/{database}/{table}")
}

/// Register a Glue connection, the resource a source leg's properties belong to.
async fn create_connection(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateConnection",
        &json!({
            "ConnectionInput": {
                "Name": name,
                "ConnectionType": "SAPODATA",
                "ConnectionProperties": { "HOST": "sap.internal" },
            },
        }),
    )
    .await;
}

/// Register a Glue database, the resource a target leg's properties belong to.
async fn create_database(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": name } }),
    )
    .await;
}

/// Register a catalog table inside `database`. Shared with
/// [`crate::integration_resource_properties`], so both integration-property suites seed a
/// table the one way.
pub(crate) async fn create_table(router: &MultiServiceRouter, database: &str, table: &str) {
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": { "Name": table, "TableType": "EXTERNAL_TABLE" },
        }),
    )
    .await;
}

/// Write one integration table property record, answering the response body.
async fn create_properties(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateIntegrationTableProperties", body).await
}

/// The `GetIntegrationTableProperties` response for one pair.
async fn get_properties(router: &MultiServiceRouter, resource_arn: &str, table: &str) -> Value {
    ok(
        router,
        "GetIntegrationTableProperties",
        &json!({ "ResourceArn": resource_arn, "TableName": table }),
    )
    .await
}

/// The status and body of a read of one pair, for the rows that must be refused.
async fn read(router: &MultiServiceRouter, resource_arn: &str, table: &str) -> (u16, Value) {
    call(
        router,
        "GetIntegrationTableProperties",
        &json!({ "ResourceArn": resource_arn, "TableName": table }),
    )
    .await
}

/// The status and body of a delete of one pair.
async fn delete_properties(
    router: &MultiServiceRouter,
    resource_arn: &str,
    table: &str,
) -> (u16, Value) {
    call(
        router,
        "DeleteIntegrationTableProperties",
        &json!({ "ResourceArn": resource_arn, "TableName": table }),
    )
    .await
}

/// The refusal wording a read or a delete of ONE record carries when it reaches none.
///
/// RECORDED template, and ONE sentence whatever the resource carries: the lookup is by the
/// PAIR the request named, so what the refusal names is that pair's record — the two members
/// it was addressed by — and not the resource, whose own existence neither call resolves.
fn miss_message(resource_arn: &str, table: &str) -> String {
    format!(
        "Failed to find the sourceTableProperties with sourceArn {resource_arn} and tableName \
         {table}"
    )
}

/// A string of exactly `length` characters starting with `prefix`, for the rows that sit on
/// a modelled length bound.
fn padded(prefix: &str, length: usize) -> String {
    let mut value = prefix.to_string();
    while value.chars().count() < length {
        value.push('x');
    }
    value
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateIntegrationTableProperties` persists is what `GetIntegrationTableProperties`
/// reports: the pair the request named addresses the record, and every member of the side
/// the create stated comes back from the store unchanged.
#[tokio::test]
async fn created_integration_table_properties_are_read_back_from_the_store() {
    let router = emulator();

    // (case, whether the ARN names a connection, the table name, the configuration sent)
    let mut cases: Vec<(String, bool, String, Value)> = vec![
        (
            // The shape a client setting up a source sends: the primary key alone, the one
            // source member the service processes.
            "a source primary key of one field".to_string(),
            true,
            "SapEntityOf".to_string(),
            json!({ "SourceTableConfig": { "PrimaryKey": ["id"] } }),
        ),
        (
            "a source primary key of several fields".to_string(),
            true,
            "sap/entity/with/slashes".to_string(),
            json!({ "SourceTableConfig": { "PrimaryKey": ["tenant", "id", "line_item"] } }),
        ),
        (
            "a target naming only its table".to_string(),
            false,
            "orders".to_string(),
            json!({ "TargetTableConfig": { "TargetTableName": "orders_replica" } }),
        ),
        (
            "a target carrying every modelled member".to_string(),
            false,
            "invoices".to_string(),
            json!({
                "TargetTableConfig": {
                    "UnnestSpec": "FULL",
                    "TargetTableName": "invoices_replica",
                    "IntegrationArn": format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:integration/zero-etl"),
                    "PartitionSpec": [
                        { "FieldName": "created_at", "FunctionSpec": "day", "ConversionSpec": "iso" },
                        { "FieldName": "region", "FunctionSpec": "identity" },
                    ],
                },
            }),
        ),
        (
            "a target partitioned on every published function".to_string(),
            false,
            "events".to_string(),
            json!({
                "TargetTableConfig": {
                    "PartitionSpec": FUNCTION_SPECS
                        .iter()
                        .map(|spec| json!({ "FieldName": format!("{spec}_field"), "FunctionSpec": spec }))
                        .collect::<Vec<Value>>(),
                },
            }),
        ),
        (
            "a table name and a target table name at the modelled length bound".to_string(),
            false,
            padded("long-", MAX_NAME_LENGTH),
            json!({
                "TargetTableConfig": { "TargetTableName": padded("long_target_", MAX_NAME_LENGTH) },
            }),
        ),
        (
            "a table name of the shortest length the model admits".to_string(),
            false,
            "t".to_string(),
            json!({ "TargetTableConfig": { "TargetTableName": "r" } }),
        ),
        (
            "a target stating no member at all".to_string(),
            false,
            "empty-config".to_string(),
            json!({ "TargetTableConfig": {} }),
        ),
    ];
    // One row per published `UnnestSpec`, the member whose value list the model states.
    for spec in UNNEST_SPECS {
        cases.push((
            format!("a target unnesting with {spec}"),
            false,
            format!("unnested-{spec}"),
            json!({ "TargetTableConfig": { "UnnestSpec": spec } }),
        ));
    }
    // One row per published `ConversionSpec`, on the timestamp function the model pairs it
    // with.
    for spec in CONVERSION_SPECS {
        cases.push((
            format!("a partition converting timestamps as {spec}"),
            false,
            format!("converted-{spec}"),
            json!({
                "TargetTableConfig": {
                    "PartitionSpec": [
                        { "FieldName": "ts", "FunctionSpec": "hour", "ConversionSpec": spec },
                    ],
                },
            }),
        ));
    }

    let mut stored: Vec<(String, String, String, Value)> = Vec::new();
    for (index, (case, is_connection, table, config)) in cases.iter().enumerate() {
        let name = format!("integration-table-{index}");
        let resource_arn = if *is_connection {
            create_connection(&router, &name).await;
            connection_arn(&name)
        } else {
            create_database(&router, &name).await;
            database_arn(&name)
        };

        let mut request = config.clone();
        request["ResourceArn"] = json!(resource_arn);
        request["TableName"] = json!(table);
        let created = create_properties(&router, &request).await;
        // `CreateIntegrationTablePropertiesResponse` models no member at all, so the create
        // reports the 200 and nothing else.
        assert_eq!(
            created,
            json!({}),
            "case {case}: the create must answer an empty body, got {created}"
        );

        let read = get_properties(&router, &resource_arn, table).await;
        assert_eq!(
            read.get("ResourceArn").and_then(Value::as_str),
            Some(resource_arn.as_str()),
            "case {case}: the read must report the resource it was asked for, got {read}"
        );
        assert_eq!(
            read.get("TableName").and_then(Value::as_str),
            Some(table.as_str()),
            "case {case}: the read must report the table it was asked for, got {read}"
        );
        assert_echoed(case, config, &read);
        // The side the create did not state carries no value.
        let unstated = if config.get("SourceTableConfig").is_some() {
            "TargetTableConfig"
        } else {
            "SourceTableConfig"
        };
        assert!(
            read.get(unstated).is_none(),
            "case {case}: {unstated} was never written, got {read}"
        );

        stored.push((case.clone(), resource_arn, table.clone(), config.clone()));
    }

    // Each pair has its OWN record: reading one back after every create has landed still
    // reports that pair's configuration, so no create overwrote another pair's record.
    for (case, resource_arn, table, config) in &stored {
        let read = get_properties(&router, resource_arn, table).await;
        assert_echoed(case, config, &read);
    }
}

/// The record is addressed by the PAIR: the same resource with another table name, and the
/// same table name under another resource, are different records.
#[tokio::test]
async fn one_record_per_resource_and_table_pair() {
    let router = emulator();
    create_database(&router, "pairs-first").await;
    create_database(&router, "pairs-second").await;

    // (case, the ARN, the table name, the target table the record carries)
    let rows = [
        ("first resource, first table", "pairs-first", "orders", "a"),
        ("first resource, second table", "pairs-first", "invoices", "b"),
        ("second resource, first table", "pairs-second", "orders", "c"),
        (
            "second resource, a table differing only in case",
            "pairs-second",
            "ORDERS",
            "d",
        ),
    ];

    for (case, database, table, target) in rows {
        create_properties(
            &router,
            &json!({
                "ResourceArn": database_arn(database),
                "TableName": table,
                "TargetTableConfig": { "TargetTableName": target },
            }),
        )
        .await;
        let read = get_properties(&router, &database_arn(database), table).await;
        assert_eq!(
            read.pointer("/TargetTableConfig/TargetTableName")
                .and_then(Value::as_str),
            Some(target),
            "case {case}: expected the target table this pair stored, got {read}"
        );
    }

    // Every row above is still readable with its own value, so the four writes landed in
    // four records.
    for (case, database, table, target) in rows {
        let read = get_properties(&router, &database_arn(database), table).await;
        assert_eq!(
            read.pointer("/TargetTableConfig/TargetTableName")
                .and_then(Value::as_str),
            Some(target),
            "case {case}: each pair keeps its own record, got {read}"
        );
    }
}

/// A create for a pair that already has a record writes THAT record: it answers 200 rather
/// than refusing a duplicate, replaces the side it states and leaves the side it does not
/// state as stored — and the store still holds one record per pair.
#[tokio::test]
async fn a_second_create_for_one_pair_writes_the_record_it_already_has() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let source = |key: &str| json!({ "SourceTableConfig": { "PrimaryKey": [key] } });
    let target = |name: &str| json!({ "TargetTableConfig": { "TargetTableName": name } });

    // (case, the first create's configuration, the second create's, what must then be read)
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "the same side with another value replaces it",
            target("first"),
            target("second"),
            target("second"),
        ),
        (
            "the other side added leaves the first side stored",
            target("kept_target"),
            source("added-key"),
            json!({
                "TargetTableConfig": { "TargetTableName": "kept_target" },
                "SourceTableConfig": { "PrimaryKey": ["added-key"] },
            }),
        ),
        (
            "a member dropped from a restated side is dropped",
            json!({
                "TargetTableConfig": { "TargetTableName": "full", "UnnestSpec": "TOPLEVEL" },
            }),
            target("narrowed"),
            json!({
                "TargetTableConfig": { "TargetTableName": "narrowed", "UnnestSpec": null },
            }),
        ),
        (
            "the identical create repeated changes nothing",
            target("idempotent"),
            target("idempotent"),
            target("idempotent"),
        ),
    ];

    for (index, (case, first, second, expected)) in cases.iter().enumerate() {
        let database = format!("upserted-{index}");
        create_database(&router, &database).await;
        let resource_arn = database_arn(&database);
        let table = format!("table-{index}");

        let mut request = first.clone();
        request["ResourceArn"] = json!(resource_arn);
        request["TableName"] = json!(table);
        create_properties(&router, &request).await;

        let mut again = second.clone();
        again["ResourceArn"] = json!(resource_arn);
        again["TableName"] = json!(table);
        let (status, body) = call(&router, "CreateIntegrationTableProperties", &again).await;
        assert_eq!(
            status, 200,
            "case {case}: a second create for one pair must answer 200, got {status} with {body}"
        );

        let read = get_properties(&router, &resource_arn, &table).await;
        assert_echoed(case, expected, &read);
    }

    // One record per pair: the second create of each row wrote the record its row's pair
    // already had rather than adding one.
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let held = state.lock().integration_table_properties.len();
    assert_eq!(
        held,
        cases.len(),
        "case every row: expected one record per pair ({}), got {held}",
        cases.len()
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One call carries the configuration of the ONE leg its `ResourceArn` names, so a call
/// stating both sides is refused with the modelled `ValidationException` and stores
/// nothing. The rows differ along how much each side carries.
#[tokio::test]
async fn a_create_stating_both_sides_is_refused() {
    let router = emulator();
    create_database(&router, "both-sides").await;
    let resource_arn = database_arn("both-sides");

    for (case, body) in [
        (
            "one member on each side",
            json!({
                "SourceTableConfig": { "PrimaryKey": ["id"] },
                "TargetTableConfig": { "TargetTableName": "replica" },
            }),
        ),
        (
            "both sides empty",
            json!({ "SourceTableConfig": {}, "TargetTableConfig": {} }),
        ),
        (
            "a full target beside a source",
            json!({
                "SourceTableConfig": { "PrimaryKey": ["id", "tenant"] },
                "TargetTableConfig": {
                    "UnnestSpec": "NOUNNEST",
                    "PartitionSpec": [{ "FieldName": "day", "FunctionSpec": "identity" }],
                },
            }),
        ),
    ] {
        let mut request = body.clone();
        request["ResourceArn"] = json!(resource_arn);
        request["TableName"] = json!("both-sides-table");
        let (status, answered) = call(&router, "CreateIntegrationTableProperties", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {request}, got {status} with {answered}"
        );
        assert!(
            error_code(&answered).contains("ValidationException"),
            "case {case}: expected ValidationException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            "Source and Target config can't both be set.",
            "case {case}: expected the service's refusal wording, got {answered}"
        );

        // The refused create stored nothing, so the read of that pair misses.
        let (status, answered) = read(&router, &resource_arn, "both-sides-table").await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create must have stored nothing, got {status} with \
             {answered}"
        );
        assert!(
            error_code(&answered).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException from the read, got {answered}"
        );
    }
}

/// A `SourceTableConfig` member the service does not process yet is refused with the
/// modelled `ValidationException`, and the one source member it DOES process is accepted.
/// The rows are the published list of unsupported members, each alone and then together.
#[tokio::test]
async fn a_source_config_stating_an_unsupported_member_is_refused() {
    let router = emulator();
    create_connection(&router, "unsupported-source").await;
    let resource_arn = connection_arn("unsupported-source");

    // Every member of the published list, spelled as the model spells it.
    let value_for = |member: &str| match member {
        "Fields" => json!(["id", "amount"]),
        "FilterPredicate" => json!("amount > 0"),
        "RecordUpdateField" => json!("last_modified"),
        other => panic!("the model lists no source member {other}"),
    };

    let mut rows: Vec<(String, Value)> = UNSUPPORTED_SOURCE_TABLE_CONFIG_MEMBERS
        .iter()
        .map(|member| {
            (
                format!("{member} alone"),
                json!({ member.to_string(): value_for(member) }),
            )
        })
        .collect();
    let mut every = serde_json::Map::new();
    for member in UNSUPPORTED_SOURCE_TABLE_CONFIG_MEMBERS {
        every.insert(member.to_string(), value_for(member));
    }
    rows.push(("every unsupported member at once".to_string(), Value::Object(every)));
    rows.push((
        "an unsupported member beside the supported one".to_string(),
        json!({ "PrimaryKey": ["id"], "FilterPredicate": "amount > 0" }),
    ));

    for (case, source_config) in &rows {
        let (status, answered) = call(
            &router,
            "CreateIntegrationTableProperties",
            &json!({
                "ResourceArn": resource_arn,
                "TableName": "unsupported-table",
                "SourceTableConfig": source_config,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {source_config}, got {status} with {answered}"
        );
        assert!(
            error_code(&answered).contains("ValidationException"),
            "case {case}: expected ValidationException, got {answered}"
        );
        assert_eq!(
            error_message(&answered),
            "Unsupported source table properties are provided.",
            "case {case}: expected the service's refusal wording, got {answered}"
        );
    }

    // The supported source member alone is stored and read back, so the rows above are
    // refused for the member they state and not for being source configurations.
    create_properties(
        &router,
        &json!({
            "ResourceArn": resource_arn,
            "TableName": "supported-table",
            "SourceTableConfig": { "PrimaryKey": ["id"] },
        }),
    )
    .await;
    let read = get_properties(&router, &resource_arn, "supported-table").await;
    assert_eq!(
        read.pointer("/SourceTableConfig/PrimaryKey/0")
            .and_then(Value::as_str),
        Some("id"),
        "case the supported member: expected the primary key this create stored, got {read}"
    );
}

/// A `ResourceArn` this operation can interpret as no resource these properties could belong
/// to is refused with the modelled `ValidationException` naming that value, and stores
/// nothing; the read and the delete of the same value answer the same sentence, because the
/// three read that member the one way. The rows differ along how the value misses.
#[tokio::test]
async fn a_resource_arn_naming_no_glue_resource_is_refused() {
    let router = emulator();

    for (case, resource_arn) in [
        ("not an ARN at all", "my-connection".to_string()),
        (
            "another service's namespace",
            "arn:aws:s3:::bucket/key".to_string(),
        ),
        (
            // The recorded case: the zero-ETL target lives in another service.
            "a Redshift Serverless namespace ARN",
            format!("arn:aws:redshift-serverless:{REGION}:{ACCOUNT_ID}:namespace/warehouse"),
        ),
        (
            "a Glue crawler ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:crawler/nightly"),
        ),
        (
            "a Glue job ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:job/etl"),
        ),
        (
            "the Data Catalog's own ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:catalog"),
        ),
        (
            "a database ARN carrying no name",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/"),
        ),
        (
            "a table ARN carrying no table after its database",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:table/sales"),
        ),
    ] {
        let expected = format!("Encountered invalid resource arn {resource_arn}");
        let (status, body) = call(
            &router,
            "CreateIntegrationTableProperties",
            &json!({
                "ResourceArn": resource_arn,
                "TableName": "orders",
                "TargetTableConfig": { "TargetTableName": "orders_replica" },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case}: expected ValidationException for {resource_arn:?}, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            error_message(&body)
        );

        for (attempt, operation) in [
            ("a read", "GetIntegrationTableProperties"),
            ("a delete", "DeleteIntegrationTableProperties"),
        ] {
            let (status, body) = call(
                &router,
                operation,
                &json!({ "ResourceArn": resource_arn, "TableName": "orders" }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}: {attempt} of {resource_arn:?} must be refused too, got {status} \
                 with {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case}: expected ValidationException from {attempt}, got {}",
                error_code(&body)
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {case}: expected {expected:?} from {attempt}, got {:?}",
                error_message(&body)
            );
        }
    }
}

/// The properties belong to the resource the ARN names, so a `ResourceArn` naming a
/// resource this account does not hold earns the modelled `EntityNotFoundException` and
/// nothing is stored for it. The rows differ along how the resource misses.
#[tokio::test]
async fn a_resource_arn_naming_an_absent_resource_is_refused() {
    let router = emulator();
    create_connection(&router, "stored-connection").await;
    create_database(&router, "stored-database").await;
    create_table(&router, "stored-database", "stored-table").await;

    for (case, resource_arn) in [
        ("a connection never created", connection_arn("absent")),
        ("a database never created", database_arn("absent")),
        (
            "a table never created in a stored database",
            table_arn("stored-database", "absent"),
        ),
        (
            "a table in a database never created",
            table_arn("absent", "stored-table"),
        ),
        (
            "a stored database's name in another account",
            format!("arn:aws:glue:{REGION}:111122223333:database/stored-database"),
        ),
        (
            "a stored connection's name as a database",
            database_arn("stored-connection"),
        ),
        (
            "a stored database's name as a connection",
            connection_arn("stored-database"),
        ),
    ] {
        let (status, body) = call(
            &router,
            "CreateIntegrationTableProperties",
            &json!({
                "ResourceArn": resource_arn,
                "TableName": "orders",
                "TargetTableConfig": { "TargetTableName": "orders_replica" },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {resource_arn:?}, got {body}"
        );
    }

    // The resources that DO exist still take a record, so the misses above are misses and
    // not a broken resolution.
    for (case, resource_arn) in [
        ("the stored connection", connection_arn("stored-connection")),
        ("the stored database", database_arn("stored-database")),
        (
            "the stored table",
            table_arn("stored-database", "stored-table"),
        ),
    ] {
        let created = create_properties(
            &router,
            &json!({
                "ResourceArn": resource_arn,
                "TableName": "orders",
                "TargetTableConfig": { "TargetTableName": "orders_replica" },
            }),
        )
        .await;
        assert_eq!(
            created,
            json!({}),
            "case {case}: the create must answer an empty body, got {created}"
        );
    }
}

/// A read reaches a record only through the ARN of an integration LEG. A create against a
/// replicated table's OWN ARN is accepted, and the record it writes is not readable through
/// that ARN; a leg's record written for another table name is not readable either. Each miss
/// carries the wording its resource earns: the pair the request addressed while that resource
/// carries properties of other tables, and the resource itself while it carries none.
#[tokio::test]
async fn a_record_is_read_back_only_through_an_integration_leg() {
    let router = emulator();
    create_database(&router, "orphans").await;
    create_table(&router, "orphans", "orders").await;
    create_connection(&router, "orphans-source").await;
    create_connection(&router, "orphans-written-source").await;

    // The create against the table's own ARN is accepted.
    let created = create_properties(
        &router,
        &json!({
            "ResourceArn": table_arn("orphans", "orders"),
            "TableName": "sap-orders",
            "TargetTableConfig": { "TargetTableName": "orders_replica" },
        }),
    )
    .await;
    assert_eq!(
        created,
        json!({}),
        "case a create against a table ARN: expected an empty body, got {created}"
    );
    // And a record was written: the store holds it under the pair the create named.
    create_properties(
        &router,
        &json!({
            "ResourceArn": database_arn("orphans"),
            "TableName": "sap-orders",
            "TargetTableConfig": { "TargetTableName": "orders_replica" },
        }),
    )
    .await;
    // One leg carrying properties of ANOTHER table, so the misses below differ along whether
    // the addressed resource carries any properties at all — which the refusal does not
    // depend on.
    create_properties(
        &router,
        &json!({
            "ResourceArn": connection_arn("orphans-written-source"),
            "TableName": "sap-invoices",
            "SourceTableConfig": { "PrimaryKey": ["id"] },
        }),
    )
    .await;

    // (case, the ARN, the table name)
    for (case, resource_arn, table) in [
        (
            "a record written against the table's own ARN",
            table_arn("orphans", "orders"),
            "sap-orders".to_string(),
        ),
        (
            "a leg with no record for this table",
            database_arn("orphans"),
            "sap-invoices".to_string(),
        ),
        (
            "a leg carrying only another table's record",
            connection_arn("orphans-written-source"),
            "sap-orders".to_string(),
        ),
        (
            "a leg that was never written to",
            connection_arn("orphans-source"),
            "sap-orders".to_string(),
        ),
        (
            "a table name differing only in case from the stored one",
            database_arn("orphans"),
            "SAP-ORDERS".to_string(),
        ),
    ] {
        let (status, body) = read(&router, &resource_arn, &table).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?} and {table:?}, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            miss_message(&resource_arn, &table),
            "case {case}: expected the wording this miss earns, got {body}"
        );
    }

    // The leg's own record is still readable, so the misses above are misses and not a
    // broken lookup.
    let read = get_properties(&router, &database_arn("orphans"), "sap-orders").await;
    assert_eq!(
        read.pointer("/TargetTableConfig/TargetTableName")
            .and_then(Value::as_str),
        Some("orders_replica"),
        "case the leg's own record: expected the stored target table, got {read}"
    );
}

/// The store holds a bounded number of records: the create at the ceiling succeeds, the one
/// past it is refused with the closest client error this operation models, a create for a
/// pair that already HAS a record still succeeds at the ceiling (the bound is on records,
/// not on writes), and freeing a slot lets a new pair take one again.
#[tokio::test]
async fn a_create_past_the_integration_table_properties_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would exercise
    // the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_INTEGRATION_TABLE_PROPERTIES - 1 {
            let input = CreateIntegrationTablePropertiesInput {
                resource_arn: database_arn("seeded"),
                table_name: format!("seeded-{index}"),
                ..CreateIntegrationTablePropertiesInput::default()
            };
            store.integration_table_properties.insert(
                (input.resource_arn.clone(), input.table_name.clone()),
                IntegrationTablePropertiesEntry::from_create_input(&input),
            );
        }
    }

    create_database(&router, "bounded").await;
    let last = json!({
        "ResourceArn": database_arn("bounded"),
        "TableName": "last-pair",
        "TargetTableConfig": { "TargetTableName": "last" },
    });
    let past = json!({
        "ResourceArn": database_arn("bounded"),
        "TableName": "one-too-many",
        "TargetTableConfig": { "TargetTableName": "past" },
    });

    create_properties(&router, &last).await;
    let (status, body) = call(&router, "CreateIntegrationTableProperties", &past).await;
    assert_eq!(
        status, 400,
        "case record {} of {MAX_INTEGRATION_TABLE_PROPERTIES}: expected 400, got {status} with \
         {body}",
        MAX_INTEGRATION_TABLE_PROPERTIES + 1
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one record too many: expected InvalidInputException, got {body}"
    );

    // A write to a record the store already holds adds nothing, so it is served at the
    // ceiling.
    create_properties(
        &router,
        &json!({
            "ResourceArn": database_arn("bounded"),
            "TableName": "last-pair",
            "TargetTableConfig": { "TargetTableName": "again" },
        }),
    )
    .await;
    let read = get_properties(&router, &database_arn("bounded"), "last-pair").await;
    assert_eq!(
        read.pointer("/TargetTableConfig/TargetTableName")
            .and_then(Value::as_str),
        Some("again"),
        "case a write at the ceiling: expected the target table the write stated, got {read}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state
            .lock()
            .integration_table_properties
            .remove(&(database_arn("seeded"), "seeded-0".to_string()));
    }
    create_properties(&router, &past).await;
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// A delete removes the ONE record the pair it names addresses: it answers an empty 200, a
/// read of that pair then misses, a second delete of it is refused and removes nothing, and
/// every other pair's record is left exactly as stored. The rows differ along which leg kind
/// the ARN names, which side of the integration the record carries, how many members that
/// side carries and the form and length of the table name.
#[tokio::test]
async fn deleting_integration_table_properties_removes_only_the_pair_it_addresses() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    create_connection(&router, "deleted-source").await;
    create_database(&router, "deleted-target").await;

    let leg_arn = |is_connection: bool| {
        if is_connection {
            connection_arn("deleted-source")
        } else {
            database_arn("deleted-target")
        }
    };
    let held = || {
        engine
            .state::<ComAmazonawsGlueAwsglueState>()
            .lock()
            .integration_table_properties
            .len()
    };

    // (case, whether the ARN names the connection leg, the table name, the configuration)
    let rows: Vec<(&str, bool, String, Value)> = vec![
        (
            "a source primary key of one field",
            true,
            "SapEntityOf".to_string(),
            json!({ "SourceTableConfig": { "PrimaryKey": ["id"] } }),
        ),
        (
            "a source primary key of several fields",
            true,
            "sap/entity/with/slashes".to_string(),
            json!({ "SourceTableConfig": { "PrimaryKey": ["tenant", "id", "line_item"] } }),
        ),
        (
            "a target naming only its table",
            false,
            "orders".to_string(),
            json!({ "TargetTableConfig": { "TargetTableName": "orders_replica" } }),
        ),
        (
            "a target carrying every modelled member",
            false,
            "invoices".to_string(),
            json!({
                "TargetTableConfig": {
                    "UnnestSpec": "NOUNNEST",
                    "TargetTableName": "invoices_replica",
                    "IntegrationArn": format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:integration/zero-etl"),
                    "PartitionSpec": [
                        { "FieldName": "created_at", "FunctionSpec": "day", "ConversionSpec": "iso" },
                    ],
                },
            }),
        ),
        (
            "a table name at the modelled length bound",
            false,
            padded("long-", MAX_NAME_LENGTH),
            json!({ "TargetTableConfig": { "TargetTableName": "long_replica" } }),
        ),
        (
            "a table name of the shortest length the model admits",
            false,
            "t".to_string(),
            json!({ "TargetTableConfig": { "TargetTableName": "r" } }),
        ),
        (
            "a target stating no member at all",
            false,
            "empty-config".to_string(),
            json!({ "TargetTableConfig": {} }),
        ),
    ];

    for (case, is_connection, table, config) in &rows {
        let resource_arn = leg_arn(*is_connection);
        let mut request = config.clone();
        request["ResourceArn"] = json!(resource_arn);
        request["TableName"] = json!(table);
        create_properties(&router, &request).await;
        // The record is there to begin with, so what the delete below removes is a stored
        // record and not an absent one.
        let read_back = get_properties(&router, &resource_arn, table).await;
        assert_echoed(case, config, &read_back);
    }

    let mut remaining: Vec<(&str, String, String, Value)> = rows
        .iter()
        .map(|(case, is_connection, table, config)| {
            (*case, leg_arn(*is_connection), table.clone(), config.clone())
        })
        .collect();

    while !remaining.is_empty() {
        let (case, resource_arn, table, _) = remaining.remove(0);
        let before = held();

        let (status, body) = delete_properties(&router, &resource_arn, &table).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 deleting {table:?} of {resource_arn:?}, got {status} with \
             {body}"
        );
        // `DeleteIntegrationTablePropertiesResponse` models no member at all, so the delete
        // reports the 200 and nothing else.
        assert_eq!(
            body,
            json!({}),
            "case {case}: the delete must answer an empty body, got {body}"
        );
        let after = held();
        assert_eq!(
            after,
            before - 1,
            "case {case}: exactly one record must be removed, the store held {before}, got \
             {after}"
        );

        // The miss of this pair now reports the record the lookup no longer finds, whether or
        // not that resource still carries properties of another table.
        let (status, miss) = read(&router, &resource_arn, &table).await;
        assert_eq!(
            status, 400,
            "case {case}: a read of the deleted pair must be refused, got {status} with {miss}"
        );
        assert!(
            error_code(&miss).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException from the read, got {miss}"
        );
        assert_eq!(
            error_message(&miss),
            miss_message(&resource_arn, &table),
            "case {case}: expected the wording this miss earns, got {miss}"
        );

        // The delete of a pair that is already gone is refused the same way and removes
        // nothing: it is not answered as though a delete had happened.
        let (status, again) = delete_properties(&router, &resource_arn, &table).await;
        assert_eq!(
            status, 400,
            "case {case}: a repeated delete must be refused, got {status} with {again}"
        );
        assert!(
            error_code(&again).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException from the repeated delete, got {again}"
        );
        assert_eq!(
            error_message(&again),
            miss_message(&resource_arn, &table),
            "case {case}: expected the wording this miss earns, got {again}"
        );
        assert_eq!(
            held(),
            after,
            "case {case}: a refused delete must remove nothing, the store held {after}, got {}",
            held()
        );

        // Every pair not yet deleted still reports what its create stored, so the delete
        // above removed one record and not a resource's worth of them.
        for (other_case, other_arn, other_table, other_config) in &remaining {
            let read_back = get_properties(&router, other_arn, other_table).await;
            assert_echoed(
                &format!("{case} deleted, then {other_case}"),
                other_config,
                &read_back,
            );
        }
    }

    let left = held();
    assert_eq!(
        left, 0,
        "case every row deleted: expected an empty store, got {left} record(s)"
    );
}

/// A delete that reaches no record is refused with the modelled `EntityNotFoundException`,
/// naming the pair it was asked for, and removes nothing. The rows differ along how
/// the record misses: another table under a written leg, a leg carrying nothing, a resource
/// this account does not hold, a record written against a replicated table's OWN ARN (which
/// is not reachable through that ARN) and a table name differing only in case.
#[tokio::test]
async fn a_delete_reaching_no_record_is_refused_and_removes_nothing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    create_database(&router, "undeleted").await;
    create_table(&router, "undeleted", "orders").await;
    create_connection(&router, "undeleted-source").await;

    // One record under the database leg, and one written against the replicated table's own
    // ARN, which the create accepts.
    for resource_arn in [table_arn("undeleted", "orders"), database_arn("undeleted")] {
        create_properties(
            &router,
            &json!({
                "ResourceArn": resource_arn,
                "TableName": "sap-orders",
                "TargetTableConfig": { "TargetTableName": "orders_replica" },
            }),
        )
        .await;
    }
    let held = || {
        engine
            .state::<ComAmazonawsGlueAwsglueState>()
            .lock()
            .integration_table_properties
            .len()
    };
    let before = held();

    // (case, the ARN, the table name)
    for (case, resource_arn, table) in [
        (
            "a leg with no record for this table",
            database_arn("undeleted"),
            "sap-invoices".to_string(),
        ),
        (
            "a table name differing only in case from the stored one",
            database_arn("undeleted"),
            "SAP-ORDERS".to_string(),
        ),
        (
            "a record written against the table's own ARN",
            table_arn("undeleted", "orders"),
            "sap-orders".to_string(),
        ),
        (
            "a leg that was never written to",
            connection_arn("undeleted-source"),
            "sap-orders".to_string(),
        ),
        (
            "a database this account does not hold",
            database_arn("absent"),
            "sap-orders".to_string(),
        ),
        (
            "a connection this account does not hold",
            connection_arn("absent"),
            "sap-orders".to_string(),
        ),
    ] {
        let (status, body) = delete_properties(&router, &resource_arn, &table).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?} and {table:?}, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            miss_message(&resource_arn, &table),
            "case {case}: expected the wording this miss earns, got {body}"
        );
        let left = held();
        assert_eq!(
            left, before,
            "case {case}: a refused delete must remove nothing, the store held {before}, got \
             {left}"
        );
    }

    // Both stored records are still there, so the refusals above are misses and not a broken
    // lookup followed by a removal.
    let read_back = get_properties(&router, &database_arn("undeleted"), "sap-orders").await;
    assert_eq!(
        read_back
            .pointer("/TargetTableConfig/TargetTableName")
            .and_then(Value::as_str),
        Some("orders_replica"),
        "case the leg's own record: expected the stored target table, got {read_back}"
    );
    assert_eq!(
        held(),
        before,
        "case every refusal: the store must still hold {before} record(s), got {}",
        held()
    );
}

/// A `ResourceArn` that can name no Glue resource integration table properties could belong
/// to is refused for the delete too, with the modelled `ValidationException` naming that
/// value, and removes nothing. The rows differ
/// along how the value misses.
#[tokio::test]
async fn a_delete_whose_resource_arn_names_no_glue_resource_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    create_database(&router, "arn-shapes").await;
    create_properties(
        &router,
        &json!({
            "ResourceArn": database_arn("arn-shapes"),
            "TableName": "orders",
            "TargetTableConfig": { "TargetTableName": "orders_replica" },
        }),
    )
    .await;
    let held = || {
        engine
            .state::<ComAmazonawsGlueAwsglueState>()
            .lock()
            .integration_table_properties
            .len()
    };

    for (case, resource_arn) in [
        ("not an ARN at all", "my-connection".to_string()),
        (
            "another service's namespace",
            "arn:aws:s3:::bucket/key".to_string(),
        ),
        (
            "a Glue crawler ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:crawler/nightly"),
        ),
        (
            "a Glue job ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:job/etl"),
        ),
        (
            "the Data Catalog's own ARN",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:catalog"),
        ),
        (
            "a database ARN carrying no name",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/"),
        ),
        (
            "a table ARN carrying no table after its database",
            format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:table/sales"),
        ),
    ] {
        let (status, body) = delete_properties(&router, &resource_arn, "orders").await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {resource_arn:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case}: expected ValidationException for {resource_arn:?}, got {}",
            error_code(&body)
        );
        let expected = format!("Encountered invalid resource arn {resource_arn}");
        assert_eq!(
            error_message(&body),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            error_message(&body)
        );
        let left = held();
        assert_eq!(
            left, 1,
            "case {case}: a refused delete must remove nothing, got {left} record(s)"
        );
    }

    // The ARN shapes above are refused for their shape: the stored pair is still deletable.
    let (status, body) = delete_properties(&router, &database_arn("arn-shapes"), "orders").await;
    assert_eq!(
        status, 200,
        "case the stored pair: expected 200, got {status} with {body}"
    );
    let left = held();
    assert_eq!(
        left, 0,
        "case the stored pair: expected the record to be gone, got {left} record(s)"
    );
}
