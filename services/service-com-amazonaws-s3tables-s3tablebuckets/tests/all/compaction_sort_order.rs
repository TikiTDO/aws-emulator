// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the precondition a compaction strategy carries:
//! which strategies a table's metadata must support before one can be
//! configured on it.
//!
//! The rule under test: a maintenance configuration naming a compaction
//! strategy that REWRITES a table's records in the table's sort order can only
//! be set on a table whose metadata declares a sort order — `sort` and
//! `z-order` of `com.amazonaws.s3tables#IcebergCompactionStrategy` do, `auto`
//! and `binpack` do not — and a configuration this refuses changes nothing:
//! the task keeps the configuration a read reported before it. The same put is
//! accepted once the table's metadata carries a sort order, whether the create
//! declared one or an Iceberg catalog commit added one.
//!
//! The table spans what the rule generalises over: every modeled compaction
//! strategy, both maintenance statuses, a table whose metadata declares a sort
//! order and one whose metadata declares none (and one with no metadata at
//! all), a settings document that names a target file size beside the strategy
//! and one that names only the strategy, and the region.

use serde_json::{json, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;

use s3tables::state::{IcebergCompactionStrategy, OpenTableFormat, TableMaintenanceType};

use super::harness::Emulator;

/// The fixed sentence a strategy that needs a sort order is refused with on a
/// table whose metadata carries none.
const NEEDS_SORT_ORDER: &str =
    "The specified compaction strategy requires sort order in the table metadata. Please refer to \
     the S3 Tables documentation for steps on configuring sort order.";

/// The strategy S3 Tables compacts a table under when no put set one, which is
/// what a read must still report after a refused put.
const DEFAULT_STRATEGY: &str = "auto";

/// The schema every table in this suite is created with: four columns, so a
/// sort order can name one of them.
fn schema() -> Value {
    json!({ "fields": [
        { "name": "user_id", "type": "string", "required": true },
        { "name": "event_type", "type": "string", "required": true },
        { "name": "event_timestamp", "type": "long", "required": true },
        { "name": "session_id", "type": "string", "required": false },
    ]})
}

/// A sort order on one column of [`schema`]. The member names are the ones the
/// model's `@jsonName` gives them (`smithy_query(query="shape",
/// shape="IcebergSortOrder,IcebergSortField")`).
fn write_order(source_id: i64, direction: &str, null_order: &str) -> Value {
    json!({ "order-id": 1, "fields": [
        {
            "source-id": source_id,
            "transform": "identity",
            "direction": direction,
            "null-order": null_order,
        },
    ]})
}

/// What a table's metadata declares when it is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Metadata {
    /// No `metadata` member at all, so the table has no metadata document.
    None,
    /// A schema and no sort order.
    SchemaOnly,
    /// A schema and a sort order.
    Sorted,
}

impl Metadata {
    /// The `metadata` member of the create, when the table declares one.
    fn member(self) -> Option<Value> {
        match self {
            Self::None => None,
            Self::SchemaOnly => Some(json!({ "iceberg": { "schema": schema() } })),
            Self::Sorted => Some(json!({ "iceberg": {
                "schema": schema(),
                "writeOrder": write_order(3, "asc", "nulls-first"),
            }})),
        }
    }

    /// Whether a table created this way carries a sort order.
    fn carries_sort_order(self) -> bool {
        self == Self::Sorted
    }
}

/// Create a table bucket, a namespace and one table whose metadata is
/// `metadata`, and answer the bucket's ARN.
async fn create_table(
    emulator: &Emulator,
    region: &str,
    bucket: &str,
    namespace: &str,
    table: &str,
    metadata: Metadata,
) -> String {
    let created = emulator
        .create_table_bucket(region, &json!({ "name": bucket }))
        .await;
    assert_eq!(
        created.status, 200,
        "creating the bucket {bucket:?} must succeed, got {status} {body}",
        status = created.status,
        body = created.body
    );
    let bucket_arn = created.text("arn");
    let namespace_created = emulator
        .create_namespace(region, &bucket_arn, &json!({ "namespace": [namespace] }))
        .await;
    assert_eq!(
        namespace_created.status, 200,
        "creating the namespace {namespace:?} must succeed, got {status} {body}",
        status = namespace_created.status,
        body = namespace_created.body
    );
    let mut request = json!({ "name": table, "format": OpenTableFormat::Iceberg.as_wire() });
    if let Some(metadata) = metadata.member() {
        request
            .as_object_mut()
            .expect("the create request is an object")
            .insert("metadata".to_string(), metadata);
    }
    let table_created = emulator
        .create_table(region, &bucket_arn, namespace, &request)
        .await;
    assert_eq!(
        table_created.status, 200,
        "creating the table {table:?} must succeed, got {status} {body}",
        status = table_created.status,
        body = table_created.body
    );
    bucket_arn
}

/// The compaction strategy a read reports for a table's compaction task.
async fn reported_strategy(
    emulator: &Emulator,
    region: &str,
    bucket_arn: &str,
    namespace: &str,
    table: &str,
    case: &str,
) -> Option<String> {
    let read = emulator
        .get_table_maintenance_configuration(region, bucket_arn, namespace, table)
        .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the read must succeed, got {status} {body}",
        status = read.status,
        body = read.body
    );
    read.body
        .get("configuration")
        .and_then(|configuration| configuration.get("icebergCompaction"))
        .and_then(|task| task.get("settings"))
        .and_then(|settings| settings.get("icebergCompaction"))
        .and_then(|compaction| compaction.get("strategy"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[tokio::test]
async fn a_strategy_that_sorts_records_needs_a_sort_order_in_the_table_metadata() {
    let emulator = Emulator::new();
    let namespace = "events";
    let table = "user_events";

    // (the region, what the table's metadata declares, the strategy, the status
    // the configuration carries, the target file size it names)
    let rows: Vec<(&str, Metadata, IcebergCompactionStrategy, &str, Option<i64>)> = vec![
        (
            "us-east-1",
            Metadata::SchemaOnly,
            IcebergCompactionStrategy::Sort,
            "enabled",
            None,
        ),
        (
            "us-west-2",
            Metadata::SchemaOnly,
            IcebergCompactionStrategy::ZOrder,
            "enabled",
            Some(256),
        ),
        // The configuration is judged by the strategy it names, so a strategy
        // the table cannot support is refused whether or not the task is on.
        (
            "eu-west-1",
            Metadata::SchemaOnly,
            IcebergCompactionStrategy::Sort,
            "disabled",
            None,
        ),
        // A table with no metadata document declares no sort order either.
        (
            "eu-central-1",
            Metadata::None,
            IcebergCompactionStrategy::Sort,
            "enabled",
            None,
        ),
        (
            "ap-south-1",
            Metadata::None,
            IcebergCompactionStrategy::ZOrder,
            "enabled",
            Some(1),
        ),
        // The strategies that reorder nothing need no sort order.
        (
            "us-east-2",
            Metadata::SchemaOnly,
            IcebergCompactionStrategy::Binpack,
            "enabled",
            Some(256),
        ),
        (
            "ca-central-1",
            Metadata::None,
            IcebergCompactionStrategy::Auto,
            "enabled",
            None,
        ),
        (
            "ap-northeast-1",
            Metadata::None,
            IcebergCompactionStrategy::Binpack,
            "disabled",
            None,
        ),
        // And every strategy is accepted on a table whose metadata declares a
        // sort order.
        (
            "us-west-1",
            Metadata::Sorted,
            IcebergCompactionStrategy::Sort,
            "enabled",
            None,
        ),
        (
            "eu-north-1",
            Metadata::Sorted,
            IcebergCompactionStrategy::ZOrder,
            "enabled",
            Some(2_147_483_647),
        ),
        (
            "sa-east-1",
            Metadata::Sorted,
            IcebergCompactionStrategy::Binpack,
            "disabled",
            Some(512),
        ),
        (
            "ap-southeast-2",
            Metadata::Sorted,
            IcebergCompactionStrategy::Auto,
            "enabled",
            None,
        ),
    ];

    for (index, (region, metadata, strategy, status, target)) in rows.into_iter().enumerate() {
        let bucket = format!("compaction-sort-{index}");
        let case = format!(
            "{region}: {strategy} on a table whose metadata is {metadata:?}, {status}",
            strategy = strategy.as_wire()
        );
        let bucket_arn =
            create_table(&emulator, region, &bucket, namespace, table, metadata).await;

        let mut compaction = json!({ "strategy": strategy.as_wire() });
        if let Some(target) = target {
            compaction
                .as_object_mut()
                .expect("the settings are an object")
                .insert("targetFileSizeMB".to_string(), json!(target));
        }
        let put = emulator
            .put_table_maintenance_configuration(
                region,
                &bucket_arn,
                namespace,
                table,
                TableMaintenanceType::IcebergCompaction.as_wire(),
                &json!({ "status": status, "settings": { "icebergCompaction": compaction } }),
            )
            .await;

        let accepted = !strategy.requires_sort_order() || metadata.carries_sort_order();
        if accepted {
            assert_eq!(
                put.status, 204,
                "case {case:?}: expected the modeled 204, got {status} {body}",
                status = put.status,
                body = put.body
            );
            let reported =
                reported_strategy(&emulator, region, &bucket_arn, namespace, table, &case).await;
            assert_eq!(
                reported.as_deref(),
                Some(strategy.as_wire()),
                "case {case:?}: expected the read to report {expected:?}, got {reported:?}",
                expected = strategy.as_wire()
            );
            continue;
        }
        assert_eq!(
            put.status, 400,
            "case {case:?}: expected the refusal 400, got {status} {body}",
            status = put.status,
            body = put.body
        );
        assert_eq!(
            put.error_code.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {actual:?}",
            actual = put.error_code
        );
        assert_eq!(
            put.message(),
            NEEDS_SORT_ORDER,
            "case {case:?}: expected {NEEDS_SORT_ORDER:?}, got {actual:?}",
            actual = put.message()
        );
        // The refusal changed nothing: the task still runs under the strategy
        // it ran under before the put.
        let reported =
            reported_strategy(&emulator, region, &bucket_arn, namespace, table, &case).await;
        assert_eq!(
            reported.as_deref(),
            Some(DEFAULT_STRATEGY),
            "case {case:?}: a refused put must leave the strategy at {DEFAULT_STRATEGY:?}, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_strategy_leaves_the_configuration_the_table_already_had() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let namespace = "events";
    let table = "user_events";
    let bucket_arn = create_table(
        &emulator,
        region,
        "compaction-sort-untouched",
        namespace,
        table,
        Metadata::SchemaOnly,
    )
    .await;

    // A configuration the table DOES support, so the refusal below has
    // something of its own to leave alone.
    let accepted = emulator
        .put_table_maintenance_configuration(
            region,
            &bucket_arn,
            namespace,
            table,
            TableMaintenanceType::IcebergCompaction.as_wire(),
            &json!({ "status": "enabled", "settings": {
                "icebergCompaction": { "strategy": "binpack", "targetFileSizeMB": 128 }
            }}),
        )
        .await;
    assert_eq!(
        accepted.status, 204,
        "the binpack put must succeed, got {status} {body}",
        status = accepted.status,
        body = accepted.body
    );

    for strategy in [
        IcebergCompactionStrategy::Sort,
        IcebergCompactionStrategy::ZOrder,
    ] {
        let case = format!("{} after binpack", strategy.as_wire());
        let refused = emulator
            .put_table_maintenance_configuration(
                region,
                &bucket_arn,
                namespace,
                table,
                TableMaintenanceType::IcebergCompaction.as_wire(),
                &json!({ "status": "enabled", "settings": {
                    "icebergCompaction": { "strategy": strategy.as_wire() }
                }}),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the refusal 400, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        let reported =
            reported_strategy(&emulator, region, &bucket_arn, namespace, table, &case).await;
        assert_eq!(
            reported.as_deref(),
            Some("binpack"),
            "case {case:?}: the refusal must leave the stored strategy alone, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_sort_order_added_by_a_catalog_commit_makes_the_strategy_configurable() {
    let emulator = Emulator::new();
    let region = "us-west-2";
    let namespace = "events";
    let table = "user_events";
    let bucket_arn = create_table(
        &emulator,
        region,
        "compaction-sort-commit",
        namespace,
        table,
        Metadata::SchemaOnly,
    )
    .await;

    let sort_put = || async {
        emulator
            .put_table_maintenance_configuration(
                region,
                &bucket_arn,
                namespace,
                table,
                TableMaintenanceType::IcebergCompaction.as_wire(),
                &json!({ "status": "enabled", "settings": {
                    "icebergCompaction": { "strategy": "sort" }
                }}),
            )
            .await
    };

    let before = sort_put().await;
    assert_eq!(
        before.status, 400,
        "before the commit: expected the refusal 400, got {status} {body}",
        status = before.status,
        body = before.body
    );

    // The sort order is added where an Iceberg engine adds one: a commit
    // through the catalog.
    let committed = emulator
        .commit_table(
            region,
            &bucket_arn,
            namespace,
            table,
            &json!({
                "updates": [
                    { "action": "add-sort-order", "sort-order": { "fields": [
                        {
                            "source-id": 3,
                            "transform": "identity",
                            "direction": "desc",
                            "null-order": "nulls-last",
                        },
                    ]}},
                    { "action": "set-default-sort-order", "sort-order-id": -1 },
                ],
            }),
        )
        .await;
    assert_eq!(
        committed.status, 200,
        "the commit must succeed, got {status} {body}",
        status = committed.status,
        body = committed.body
    );

    let after = sort_put().await;
    assert_eq!(
        after.status, 204,
        "after the commit: expected the modeled 204, got {status} {body}",
        status = after.status,
        body = after.body
    );
    let reported = reported_strategy(
        &emulator,
        region,
        &bucket_arn,
        namespace,
        table,
        "after the commit",
    )
    .await;
    assert_eq!(
        reported.as_deref(),
        Some("sort"),
        "after the commit: expected the read to report \"sort\", got {reported:?}"
    );
}
