// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the two table maintenance operations: the read
//! that reports a table's maintenance configuration and the put that sets one.
//!
//! The rule under test: S3 Tables maintains every table from the moment it is
//! created, so the read reports ONE entry per modeled `TableMaintenanceType`
//! for every resident table — the configuration a put persisted for that task,
//! with every member the put left out filled in from the task's service
//! default, and the task's whole default for a task no put ever touched — and a
//! put replaces the configuration of the task it addresses and no other. The
//! read writes nothing, and a call against a table (or a table bucket) the
//! store does not hold is refused with the modeled not-found.
//!
//! The table spans what the rule generalises over: which maintenance task is
//! addressed, the status the configuration carries, each settings member
//! (including the modeled `@range(1, 2147483647)` bounds and a value past
//! them), a configuration with no settings at all, a configuration with no
//! status, the compaction strategy, the region, and whether the task was ever
//! configured.

use serde_json::{json, Map, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;

use s3tables::state::{
    ComAmazonawsS3tablesS3tablebucketsState as S3TablesState, OpenTableFormat, TableMaintenanceType,
};

use super::harness::{Emulator, Reply};

/// The fixed sentence this service states a table the store does not hold with.
const NO_TABLE: &str = "The specified table does not exist.";

/// The fixed sentence this service states a table bucket the store does not
/// hold with.
const NO_BUCKET: &str = "The specified bucket does not exist.";

/// The settings S3 Tables compacts a table under when nothing configured it,
/// from the S3 Tables maintenance documentation: compaction is on, it rewrites
/// towards 512 MB files, and the strategy is derived from the table's own sort
/// order. Written out here rather than read from the service's own constants,
/// so the test states the published figures independently.
const DEFAULT_COMPACTION: (&str, i64, &str) = ("enabled", 512, "auto");

/// The settings S3 Tables manages a table's snapshots under when nothing
/// configured it, from the same documentation: snapshot management is on, at
/// least one snapshot is kept and a snapshot is expired once it is older than
/// 120 hours.
const DEFAULT_SNAPSHOT_MANAGEMENT: (&str, i64, i64) = ("enabled", 1, 120);

/// One maintenance configuration as a read reports it, reduced to the members
/// the model declares for the task it belongs to.
#[derive(Debug, PartialEq, Eq)]
enum Reported {
    /// `status`, `settings.icebergCompaction.targetFileSizeMB` and
    /// `settings.icebergCompaction.strategy`.
    Compaction(Option<String>, Option<i64>, Option<String>),
    /// `status`, `settings.icebergSnapshotManagement.minSnapshotsToKeep` and
    /// `settings.icebergSnapshotManagement.maxSnapshotAgeHours`.
    SnapshotManagement(Option<String>, Option<i64>, Option<i64>),
}

impl Reported {
    /// The compaction configuration every table runs under without one.
    fn default_compaction() -> Self {
        let (status, target, strategy) = DEFAULT_COMPACTION;
        Self::Compaction(
            Some(status.to_string()),
            Some(target),
            Some(strategy.to_string()),
        )
    }

    /// The snapshot management configuration every table runs under without
    /// one.
    fn default_snapshot_management() -> Self {
        let (status, minimum, age) = DEFAULT_SNAPSHOT_MANAGEMENT;
        Self::SnapshotManagement(Some(status.to_string()), Some(minimum), Some(age))
    }
}

/// Create a table bucket, a namespace and one table in it, and answer the
/// bucket's ARN with the table's ARN.
///
/// The table is created WITH a schema and a sort order, so every modeled
/// compaction strategy can be configured on it: a strategy that rewrites
/// records in the table's sort order is refused on a table whose metadata
/// carries none, which is the rule `compaction_sort_order.rs` covers.
async fn create_table(
    emulator: &Emulator,
    region: &str,
    bucket: &str,
    namespace: &str,
    table: &str,
) -> (String, String) {
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
    let table_created = emulator
        .create_table(
            region,
            &bucket_arn,
            namespace,
            &json!({
                "name": table,
                "format": OpenTableFormat::Iceberg.as_wire(),
                "metadata": { "iceberg": {
                    "schema": { "fields": [
                        { "name": "id", "type": "long", "required": true },
                        { "name": "recorded_at", "type": "timestamp", "required": false },
                    ]},
                    // `IcebergSortOrder` and `IcebergSortField` carry their
                    // members under the names the model's `@jsonName` gives
                    // them (`smithy_query(query="shape",
                    // shape="IcebergSortOrder,IcebergSortField")`).
                    "writeOrder": { "order-id": 1, "fields": [
                        {
                            "source-id": 2,
                            "transform": "identity",
                            "direction": "asc",
                            "null-order": "nulls-first",
                        },
                    ]},
                }},
            }),
        )
        .await;
    assert_eq!(
        table_created.status, 200,
        "creating the table {table:?} must succeed, got {status} {body}",
        status = table_created.status,
        body = table_created.body
    );
    (bucket_arn, table_created.text("tableARN"))
}

/// The whole `configuration` map a read reported, one entry per task, in key
/// order — the response's map has no order of its own, so the rows are sorted
/// here rather than compared in whatever order they arrived in.
fn reported(reply: &Reply, case: &str) -> Vec<(String, Reported)> {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the read must succeed, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    let configuration = reply
        .body
        .get("configuration")
        .and_then(Value::as_object)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the read has no configuration map: {body}",
                body = reply.body
            )
        });
    let mut rows = configuration
        .iter()
        .map(|(kind, value)| {
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string);
            let settings = value.get("settings");
            let member = |task: &str, member: &str| {
                settings
                    .and_then(|settings| settings.get(task))
                    .and_then(|task| task.get(member))
                    .cloned()
            };
            let reported = match kind.as_str() {
                "icebergCompaction" => Reported::Compaction(
                    status,
                    member("icebergCompaction", "targetFileSizeMB").and_then(|it| it.as_i64()),
                    member("icebergCompaction", "strategy")
                        .and_then(|it| it.as_str().map(str::to_string)),
                ),
                "icebergSnapshotManagement" => Reported::SnapshotManagement(
                    status,
                    member("icebergSnapshotManagement", "minSnapshotsToKeep")
                        .and_then(|it| it.as_i64()),
                    member("icebergSnapshotManagement", "maxSnapshotAgeHours")
                        .and_then(|it| it.as_i64()),
                ),
                other => panic!(
                    "case {case:?}: the read reported the unmodeled maintenance type {other:?}: \
                     {body}",
                    body = reply.body
                ),
            };
            (kind.clone(), reported)
        })
        .collect::<Vec<(String, Reported)>>();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

/// The configuration a read reported for ONE task.
fn reported_task(reply: &Reply, case: &str, kind: TableMaintenanceType) -> Reported {
    reported(reply, case)
        .into_iter()
        .find(|(reported_kind, _)| reported_kind == kind.as_wire())
        .map(|(_, value)| value)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the read reported no {kind} entry: {body}",
                kind = kind.as_wire(),
                body = reply.body
            )
        })
}

/// How many maintenance configurations the store holds for `table_arn` — a put
/// persists one per task it was addressed with, and nothing else does.
fn stored_tasks(emulator: &Emulator, table_arn: &str) -> Vec<String> {
    let store = emulator.engine.state::<S3TablesState>();
    let store = store.lock();
    store
        .table_by_arn(table_arn)
        .map(|table| {
            table
                .maintenance
                .keys()
                .map(|kind| kind.as_wire().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the put table: the task addressed, the configuration sent, and
/// what the read must report for that task afterwards.
struct PutCase {
    label: &'static str,
    region: &'static str,
    kind: TableMaintenanceType,
    /// The `value.status` member, absent when the row carries none.
    status: Option<&'static str>,
    /// The `value.settings` member, absent when the row carries none.
    settings: Option<Value>,
    expected: Reported,
}

impl PutCase {
    /// The `value` member of a `PutTableMaintenanceConfiguration` request.
    fn value(&self) -> Value {
        let mut value = Map::new();
        if let Some(status) = self.status {
            value.insert("status".to_string(), json!(status));
        }
        if let Some(settings) = &self.settings {
            value.insert("settings".to_string(), settings.clone());
        }
        Value::Object(value)
    }
}

/// The rows the put is graded on. Every settings member the model declares
/// appears, each modeled `IcebergCompactionStrategy` value is covered
/// (`com.amazonaws.s3tables#IcebergCompactionStrategy`: auto, binpack, sort,
/// z-order), and so are both `MaintenanceStatus` values and the `@range(1,
/// 2147483647)` bounds of `PositiveInteger`.
fn put_cases() -> Vec<PutCase> {
    let (_, default_target, default_strategy) = DEFAULT_COMPACTION;
    let (_, default_minimum, default_age) = DEFAULT_SNAPSHOT_MANAGEMENT;
    vec![
        PutCase {
            label: "compaction turned off, settings and all",
            region: "us-east-1",
            kind: TableMaintenanceType::IcebergCompaction,
            status: Some("disabled"),
            settings: Some(json!({
                "icebergCompaction": { "targetFileSizeMB": 128, "strategy": "binpack" }
            })),
            expected: Reported::Compaction(
                Some("disabled".to_string()),
                Some(128),
                Some("binpack".to_string()),
            ),
        },
        PutCase {
            label: "compaction at the smallest target file size the model allows",
            region: "eu-west-1",
            kind: TableMaintenanceType::IcebergCompaction,
            status: Some("enabled"),
            settings: Some(json!({
                "icebergCompaction": { "targetFileSizeMB": 1, "strategy": "sort" }
            })),
            expected: Reported::Compaction(
                Some("enabled".to_string()),
                Some(1),
                Some("sort".to_string()),
            ),
        },
        PutCase {
            label: "compaction at the largest target file size the model allows",
            region: "ap-south-1",
            kind: TableMaintenanceType::IcebergCompaction,
            status: Some("enabled"),
            settings: Some(json!({
                "icebergCompaction": { "targetFileSizeMB": 2147483647, "strategy": "z-order" }
            })),
            expected: Reported::Compaction(
                Some("enabled".to_string()),
                Some(2_147_483_647),
                Some("z-order".to_string()),
            ),
        },
        PutCase {
            label: "compaction with a strategy and no target file size",
            region: "us-west-2",
            kind: TableMaintenanceType::IcebergCompaction,
            status: Some("enabled"),
            settings: Some(json!({ "icebergCompaction": { "strategy": "auto" } })),
            expected: Reported::Compaction(
                Some("enabled".to_string()),
                // The member the put left out is the service's own default.
                Some(default_target),
                Some("auto".to_string()),
            ),
        },
        PutCase {
            label: "compaction with a status and no settings at all",
            region: "us-east-1",
            kind: TableMaintenanceType::IcebergCompaction,
            status: Some("disabled"),
            settings: None,
            expected: Reported::Compaction(
                Some("disabled".to_string()),
                Some(default_target),
                Some(default_strategy.to_string()),
            ),
        },
        PutCase {
            label: "snapshot management turned off, settings and all",
            region: "eu-central-1",
            kind: TableMaintenanceType::IcebergSnapshotManagement,
            status: Some("disabled"),
            settings: Some(json!({
                "icebergSnapshotManagement": {
                    "minSnapshotsToKeep": 5,
                    "maxSnapshotAgeHours": 24
                }
            })),
            expected: Reported::SnapshotManagement(
                Some("disabled".to_string()),
                Some(5),
                Some(24),
            ),
        },
        PutCase {
            label: "snapshot management at both bounds the model allows",
            region: "us-east-2",
            kind: TableMaintenanceType::IcebergSnapshotManagement,
            status: Some("enabled"),
            settings: Some(json!({
                "icebergSnapshotManagement": {
                    "minSnapshotsToKeep": 1,
                    "maxSnapshotAgeHours": 2147483647
                }
            })),
            expected: Reported::SnapshotManagement(
                Some("enabled".to_string()),
                Some(1),
                Some(2_147_483_647),
            ),
        },
        PutCase {
            label: "snapshot management with one settings member only",
            region: "us-east-1",
            kind: TableMaintenanceType::IcebergSnapshotManagement,
            status: Some("enabled"),
            settings: Some(json!({
                "icebergSnapshotManagement": { "minSnapshotsToKeep": 7 }
            })),
            expected: Reported::SnapshotManagement(
                Some("enabled".to_string()),
                Some(7),
                Some(default_age),
            ),
        },
        PutCase {
            label: "snapshot management settings with no status",
            region: "ca-central-1",
            kind: TableMaintenanceType::IcebergSnapshotManagement,
            status: None,
            settings: Some(json!({
                "icebergSnapshotManagement": { "maxSnapshotAgeHours": 6 }
            })),
            expected: Reported::SnapshotManagement(
                // The status the put left out is the service's own.
                Some(DEFAULT_SNAPSHOT_MANAGEMENT.0.to_string()),
                Some(default_minimum),
                Some(6),
            ),
        },
    ]
}

#[tokio::test]
async fn a_new_table_is_maintained_under_the_published_defaults() {
    let emulator = Emulator::new();

    // The rule holds for every table, so the rows differ in the region, the
    // bucket, the namespace and the table name.
    let rows = [
        ("us-east-1", "maintenance-defaults-one", "sales", "orders"),
        (
            "eu-west-1",
            "maintenance-defaults-two",
            "analytics_2026",
            "page_views_by_hour",
        ),
        ("ap-northeast-1", "maintenance-defaults-3", "x", "y"),
    ];

    for (region, bucket, namespace, table) in rows {
        let case = format!("{region}/{bucket}/{namespace}/{table}");
        let (bucket_arn, table_arn) =
            create_table(&emulator, region, bucket, namespace, table).await;

        let read = emulator
            .get_table_maintenance_configuration(region, &bucket_arn, namespace, table)
            .await;
        assert_eq!(
            read.text("tableARN"),
            table_arn,
            "case {case:?}: expected the table's own ARN {table_arn:?}, got {actual:?}",
            actual = read.text("tableARN")
        );

        // One entry per modeled maintenance type, each carrying the published
        // default for that task. Both sides are in key order, so the comparison
        // is about the entries and not about the order they arrived in.
        let mut expected: Vec<(String, Reported)> = vec![
            (
                TableMaintenanceType::IcebergCompaction.as_wire().to_string(),
                Reported::default_compaction(),
            ),
            (
                TableMaintenanceType::IcebergSnapshotManagement
                    .as_wire()
                    .to_string(),
                Reported::default_snapshot_management(),
            ),
        ];
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        let actual = reported(&read, &case);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the published defaults {expected:?}, got {actual:?}"
        );

        // A read reports what is in force without persisting a configuration,
        // so a later caller sees exactly what this one did.
        assert_eq!(
            stored_tasks(&emulator, &table_arn),
            Vec::<String>::new(),
            "case {case:?}: the read must persist no configuration, got {actual:?}",
            actual = stored_tasks(&emulator, &table_arn)
        );
        let again = emulator
            .get_table_maintenance_configuration(region, &bucket_arn, namespace, table)
            .await;
        assert_eq!(
            reported(&again, &case),
            reported(&read, &case),
            "case {case:?}: a second read must report the same configuration"
        );
    }
}

#[tokio::test]
async fn the_read_reports_the_configuration_a_put_persisted() {
    let emulator = Emulator::new();

    for (index, case) in put_cases().into_iter().enumerate() {
        let bucket = format!("maintenance-put-{index}");
        let namespace = "ns";
        let table = "t";
        let (bucket_arn, table_arn) =
            create_table(&emulator, case.region, &bucket, namespace, table).await;

        let put = emulator
            .put_table_maintenance_configuration(
                case.region,
                &bucket_arn,
                namespace,
                table,
                case.kind.as_wire(),
                &case.value(),
            )
            .await;
        assert_eq!(
            put.status, 204,
            "case {label:?}: the put must answer the modeled 204, got {status} {body}",
            label = case.label,
            status = put.status,
            body = put.body
        );
        assert_eq!(
            put.body,
            Value::Null,
            "case {label:?}: the put must carry no body, got {body}",
            label = case.label,
            body = put.body
        );
        assert_eq!(
            stored_tasks(&emulator, &table_arn),
            vec![case.kind.as_wire().to_string()],
            "case {label:?}: the put must persist exactly the task it addressed, got {actual:?}",
            label = case.label,
            actual = stored_tasks(&emulator, &table_arn)
        );

        let read = emulator
            .get_table_maintenance_configuration(case.region, &bucket_arn, namespace, table)
            .await;
        let actual = reported_task(&read, case.label, case.kind);
        assert_eq!(
            actual, case.expected,
            "case {label:?}: expected {expected:?}, got {actual:?}",
            label = case.label,
            expected = case.expected
        );

        // The put settles ONE task: every other task still runs under its own
        // default.
        for other in TableMaintenanceType::ALL
            .iter()
            .copied()
            .filter(|kind| *kind != case.kind)
        {
            let expected = match other {
                TableMaintenanceType::IcebergCompaction => Reported::default_compaction(),
                TableMaintenanceType::IcebergSnapshotManagement => {
                    Reported::default_snapshot_management()
                }
            };
            let untouched = reported_task(&read, case.label, other);
            assert_eq!(
                untouched, expected,
                "case {label:?}: {kind} was not addressed, so expected {expected:?}, got \
                 {untouched:?}",
                label = case.label,
                kind = other.as_wire()
            );
        }

        // The put replaces the whole configuration of its task: a second put of
        // another configuration for the same task leaves no trace of the first.
        let replacing = json!({ "status": "enabled" });
        let replaced = emulator
            .put_table_maintenance_configuration(
                case.region,
                &bucket_arn,
                namespace,
                table,
                case.kind.as_wire(),
                &replacing,
            )
            .await;
        assert_eq!(
            replaced.status, 204,
            "case {label:?}: the replacing put must succeed, got {status} {body}",
            label = case.label,
            status = replaced.status,
            body = replaced.body
        );
        let after = reported_task(
            &emulator
                .get_table_maintenance_configuration(case.region, &bucket_arn, namespace, table)
                .await,
            case.label,
            case.kind,
        );
        let expected_after = match case.kind {
            TableMaintenanceType::IcebergCompaction => Reported::default_compaction(),
            TableMaintenanceType::IcebergSnapshotManagement => {
                Reported::default_snapshot_management()
            }
        };
        assert_eq!(
            after, expected_after,
            "case {label:?}: a put must replace the whole configuration of its task, expected \
             {expected_after:?}, got {after:?}",
            label = case.label
        );
    }
}

#[tokio::test]
async fn each_table_carries_its_own_maintenance_configuration() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let namespace = "siblings";
    let (bucket_arn, _) =
        create_table(&emulator, region, "maintenance-siblings", namespace, "left").await;
    let created_right = emulator
        .create_table(
            region,
            &bucket_arn,
            namespace,
            &json!({ "name": "right", "format": OpenTableFormat::Iceberg.as_wire() }),
        )
        .await;
    assert_eq!(
        created_right.status, 200,
        "creating the sibling table must succeed, got {status} {body}",
        status = created_right.status,
        body = created_right.body
    );

    let turned_off = emulator
        .put_table_maintenance_configuration(
            region,
            &bucket_arn,
            namespace,
            "left",
            TableMaintenanceType::IcebergCompaction.as_wire(),
            &json!({ "status": "disabled", "settings": {
                "icebergCompaction": { "targetFileSizeMB": 64, "strategy": "binpack" }
            }}),
        )
        .await;
    assert_eq!(
        turned_off.status, 204,
        "the put on the left table must succeed, got {status} {body}",
        status = turned_off.status,
        body = turned_off.body
    );

    for (case, table, expected) in [
        (
            "the table the put addressed",
            "left",
            Reported::Compaction(
                Some("disabled".to_string()),
                Some(64),
                Some("binpack".to_string()),
            ),
        ),
        (
            "its sibling in the same namespace",
            "right",
            Reported::default_compaction(),
        ),
    ] {
        let read = emulator
            .get_table_maintenance_configuration(region, &bucket_arn, namespace, table)
            .await;
        let actual = reported_task(&read, case, TableMaintenanceType::IcebergCompaction);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_configuration_the_model_refuses_is_not_persisted() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let namespace = "refusals";
    let table = "t";
    let (bucket_arn, table_arn) =
        create_table(&emulator, region, "maintenance-refusals", namespace, table).await;

    // `PositiveInteger` is `@range(1, 2147483647)`, so each settings member is
    // refused one step past its bounds; the settings union also names the task
    // it configures, so settings of the other task are not a configuration of
    // the one addressed.
    //
    // The last element is the error code the refusal must carry, where THIS
    // operation is the layer that refuses: the handler words a configuration it
    // will not store as the modeled `BadRequestException`. The rows the
    // constraint boundary refuses before the handler runs are graded on the
    // status and on nothing being persisted — which code and text a constraint
    // violation carries is one rule for every operation of this service, and
    // belongs to the suite that covers it (`constraint_refusals.rs`), not here.
    let refused: Vec<(&str, TableMaintenanceType, Value, Option<&str>)> = vec![
        (
            "a target file size below the modeled minimum",
            TableMaintenanceType::IcebergCompaction,
            json!({ "status": "enabled", "settings": {
                "icebergCompaction": { "targetFileSizeMB": 0 }
            }}),
            None,
        ),
        (
            "a negative target file size",
            TableMaintenanceType::IcebergCompaction,
            json!({ "status": "enabled", "settings": {
                "icebergCompaction": { "targetFileSizeMB": -1 }
            }}),
            None,
        ),
        (
            "a compaction strategy the model does not declare",
            TableMaintenanceType::IcebergCompaction,
            json!({ "status": "enabled", "settings": {
                "icebergCompaction": { "strategy": "interleaved" }
            }}),
            None,
        ),
        (
            "a snapshot minimum below the modeled minimum",
            TableMaintenanceType::IcebergSnapshotManagement,
            json!({ "status": "enabled", "settings": {
                "icebergSnapshotManagement": { "minSnapshotsToKeep": 0 }
            }}),
            None,
        ),
        (
            "a snapshot age past the modeled maximum",
            TableMaintenanceType::IcebergSnapshotManagement,
            json!({ "status": "enabled", "settings": {
                "icebergSnapshotManagement": { "maxSnapshotAgeHours": 2147483648i64 }
            }}),
            None,
        ),
        (
            "a status the model does not declare",
            TableMaintenanceType::IcebergSnapshotManagement,
            json!({ "status": "paused" }),
            None,
        ),
        (
            "settings of the other maintenance task",
            TableMaintenanceType::IcebergCompaction,
            json!({ "status": "enabled", "settings": {
                "icebergSnapshotManagement": { "minSnapshotsToKeep": 3 }
            }}),
            // Refused by the handler itself, so the modeled client-input error
            // is part of what this suite grades.
            Some("BadRequestException"),
        ),
    ];

    for (case, kind, value, expected_code) in refused {
        let reply = emulator
            .put_table_maintenance_configuration(
                region,
                &bucket_arn,
                namespace,
                table,
                kind.as_wire(),
                &value,
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected the client-input refusal 400, got {status} {body}",
            status = reply.status,
            body = reply.body
        );
        if let Some(code) = expected_code {
            assert_eq!(
                reply.error_code.as_deref(),
                Some(code),
                "case {case:?}: expected {code}, got {actual:?}",
                actual = reply.error_code
            );
        }
        assert_eq!(
            stored_tasks(&emulator, &table_arn),
            Vec::<String>::new(),
            "case {case:?}: a refused put must persist nothing, got {actual:?}",
            actual = stored_tasks(&emulator, &table_arn)
        );
    }

    // And the table still reads back under the defaults, so the refusals above
    // changed nothing about it.
    let read = emulator
        .get_table_maintenance_configuration(region, &bucket_arn, namespace, table)
        .await;
    assert_eq!(
        reported_task(
            &read,
            "after the refusals",
            TableMaintenanceType::IcebergCompaction
        ),
        Reported::default_compaction(),
        "after the refusals: the table must still run under the published default"
    );
}

#[tokio::test]
async fn a_call_against_a_resource_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let namespace = "present";
    let table = "t";
    let (bucket_arn, _) =
        create_table(&emulator, region, "maintenance-present", namespace, table).await;

    // (case, the bucket ARN addressed, the namespace, the table name, the fixed
    // sentence the refusal must carry)
    let cases: Vec<(&str, String, &str, &str, &str)> = vec![
        (
            "a table bucket that was never created",
            "arn:aws:s3tables:us-east-1:000000000000:bucket/maintenance-absent".to_string(),
            namespace,
            table,
            NO_BUCKET,
        ),
        (
            "the resident bucket's name in another account",
            bucket_arn.replace("000000000000", "123456789012"),
            namespace,
            table,
            NO_BUCKET,
        ),
        (
            "a namespace the bucket does not hold",
            bucket_arn.clone(),
            "absent_ns",
            table,
            NO_TABLE,
        ),
        (
            "a table the namespace does not hold",
            bucket_arn.clone(),
            namespace,
            "absent_table",
            NO_TABLE,
        ),
    ];

    for (case, arn, requested_namespace, requested_table, message) in cases {
        for (operation, reply) in [
            (
                "GetTableMaintenanceConfiguration",
                emulator
                    .get_table_maintenance_configuration(
                        region,
                        &arn,
                        requested_namespace,
                        requested_table,
                    )
                    .await,
            ),
            (
                "PutTableMaintenanceConfiguration",
                emulator
                    .put_table_maintenance_configuration(
                        region,
                        &arn,
                        requested_namespace,
                        requested_table,
                        TableMaintenanceType::IcebergCompaction.as_wire(),
                        &json!({ "status": "disabled" }),
                    )
                    .await,
            ),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case:?} on {operation}: expected 404, got {status} {body}",
                status = reply.status,
                body = reply.body
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("NotFoundException"),
                "case {case:?} on {operation}: expected NotFoundException, got {actual:?}",
                actual = reply.error_code
            );
            assert_eq!(
                reply.message(),
                message,
                "case {case:?} on {operation}: expected {message:?}, got {actual:?}",
                actual = reply.message()
            );
        }
    }

    // The resident table still answers, so the refusals above are about what
    // each named and not about an empty store.
    let read = emulator
        .get_table_maintenance_configuration(region, &bucket_arn, namespace, table)
        .await;
    assert_eq!(
        read.status, 200,
        "the resident table must still answer, got {status} {body}",
        status = read.status,
        body = read.body
    );
}
