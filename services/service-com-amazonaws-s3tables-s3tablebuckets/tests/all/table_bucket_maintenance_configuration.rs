// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the two table bucket maintenance operations: the
//! read that reports a bucket's maintenance configuration and the put that sets
//! one.
//!
//! The rule under test: S3 Tables maintains every table bucket from the moment
//! it is created, so the read reports ONE entry per modeled
//! `TableBucketMaintenanceType` for every resident bucket — the configuration a
//! put persisted for that task, with every member the put left out filled in
//! from the task's service default, and the task's whole default for a task no
//! put ever touched — and a put replaces the configuration of the task it
//! addresses and no other. The read writes nothing, and a call against a table
//! bucket the store does not hold is refused with the modeled not-found.
//!
//! The table spans what the rule generalises over: the status the configuration
//! carries, each settings member (including the modeled `@range(1,
//! 2147483647)` bounds of `PositiveInteger` and a value past them), a
//! configuration with no settings at all, a configuration with no status, the
//! region, which bucket is addressed, and whether the task was ever configured.

use serde_json::{json, Map, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;

use s3tables::state::{
    ComAmazonawsS3tablesS3tablebucketsState as S3TablesState, TableBucketMaintenanceType,
};

use super::harness::{Emulator, Reply};

/// The fixed sentence this service states a table bucket the store does not
/// hold with.
const NO_BUCKET: &str = "The specified bucket does not exist.";

/// The settings S3 Tables removes a table bucket's unreferenced files under
/// when nothing configured it, from the S3 Tables maintenance documentation:
/// unreferenced file removal is on, a file is marked noncurrent once it has
/// been unreferenced for 3 days, and a noncurrent file is deleted after 10
/// days. Written out here rather than read from the service's own constants, so
/// the test states the published figures independently.
const DEFAULT_UNREFERENCED_FILE_REMOVAL: (&str, i64, i64) = ("enabled", 3, 10);

/// One maintenance configuration as a read reports it, reduced to the members
/// the model declares for the task it belongs to: `status`,
/// `settings.icebergUnreferencedFileRemoval.unreferencedDays` and
/// `settings.icebergUnreferencedFileRemoval.nonCurrentDays`.
#[derive(Debug, PartialEq, Eq)]
struct Reported(Option<String>, Option<i64>, Option<i64>);

impl Reported {
    /// The unreferenced file removal configuration every table bucket runs
    /// under without one.
    fn default_unreferenced_file_removal() -> Self {
        let (status, unreferenced, non_current) = DEFAULT_UNREFERENCED_FILE_REMOVAL;
        Self(Some(status.to_string()), Some(unreferenced), Some(non_current))
    }
}

/// Create a table bucket and answer its ARN.
async fn create_bucket(emulator: &Emulator, region: &str, bucket: &str) -> String {
    let created = emulator
        .create_table_bucket(region, &json!({ "name": bucket }))
        .await;
    assert_eq!(
        created.status, 200,
        "creating the bucket {bucket:?} must succeed, got {status} {body}",
        status = created.status,
        body = created.body
    );
    created.text("arn")
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
            let member = |member: &str| {
                value
                    .get("settings")
                    .and_then(|settings| settings.get("icebergUnreferencedFileRemoval"))
                    .and_then(|task| task.get(member))
                    .and_then(|it| it.as_i64())
            };
            assert_eq!(
                kind.as_str(),
                TableBucketMaintenanceType::IcebergUnreferencedFileRemoval.as_wire(),
                "case {case:?}: the read reported the unmodeled maintenance type {kind:?}: \
                 {body}",
                body = reply.body
            );
            (
                kind.clone(),
                Reported(status, member("unreferencedDays"), member("nonCurrentDays")),
            )
        })
        .collect::<Vec<(String, Reported)>>();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

/// The configuration a read reported for ONE task.
fn reported_task(reply: &Reply, case: &str, kind: TableBucketMaintenanceType) -> Reported {
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

/// Which maintenance configurations the store holds for `bucket_arn` — a put
/// persists one per task it was addressed with, and nothing else does.
fn stored_tasks(emulator: &Emulator, bucket_arn: &str) -> Vec<String> {
    let store = emulator.engine.state::<S3TablesState>();
    let store = store.lock();
    store
        .table_buckets
        .get(bucket_arn)
        .map(|bucket| {
            bucket
                .maintenance
                .keys()
                .map(|kind| kind.as_wire().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the put table: the configuration sent, and what the read must
/// report for the task afterwards.
struct PutCase {
    label: &'static str,
    region: &'static str,
    /// The `value.status` member, absent when the row carries none.
    status: Option<&'static str>,
    /// The `value.settings` member, absent when the row carries none.
    settings: Option<Value>,
    expected: Reported,
}

impl PutCase {
    /// The `value` member of a `PutTableBucketMaintenanceConfiguration`
    /// request.
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

/// The rows the put is graded on. Every settings member the model declares for
/// `IcebergUnreferencedFileRemovalSettings` appears, both `MaintenanceStatus`
/// values are covered (`com.amazonaws.s3tables#MaintenanceStatus`: enabled,
/// disabled) and so are the `@range(1, 2147483647)` bounds of
/// `PositiveInteger`.
fn put_cases() -> Vec<PutCase> {
    let (_, default_unreferenced, default_non_current) = DEFAULT_UNREFERENCED_FILE_REMOVAL;
    vec![
        PutCase {
            label: "unreferenced file removal turned off, settings and all",
            region: "us-east-1",
            status: Some("disabled"),
            settings: Some(json!({
                "icebergUnreferencedFileRemoval": {
                    "unreferencedDays": 7,
                    "nonCurrentDays": 21
                }
            })),
            expected: Reported(Some("disabled".to_string()), Some(7), Some(21)),
        },
        PutCase {
            label: "both settings members at the smallest value the model allows",
            region: "eu-west-1",
            status: Some("enabled"),
            settings: Some(json!({
                "icebergUnreferencedFileRemoval": {
                    "unreferencedDays": 1,
                    "nonCurrentDays": 1
                }
            })),
            expected: Reported(Some("enabled".to_string()), Some(1), Some(1)),
        },
        PutCase {
            label: "both settings members at the largest value the model allows",
            region: "ap-south-1",
            status: Some("enabled"),
            settings: Some(json!({
                "icebergUnreferencedFileRemoval": {
                    "unreferencedDays": 2147483647,
                    "nonCurrentDays": 2147483647
                }
            })),
            expected: Reported(
                Some("enabled".to_string()),
                Some(2_147_483_647),
                Some(2_147_483_647),
            ),
        },
        PutCase {
            label: "one settings member only",
            region: "us-west-2",
            status: Some("enabled"),
            settings: Some(json!({
                "icebergUnreferencedFileRemoval": { "unreferencedDays": 30 }
            })),
            expected: Reported(
                Some("enabled".to_string()),
                Some(30),
                // The member the put left out is the service's own default.
                Some(default_non_current),
            ),
        },
        PutCase {
            label: "the other settings member only",
            region: "eu-central-1",
            status: Some("disabled"),
            settings: Some(json!({
                "icebergUnreferencedFileRemoval": { "nonCurrentDays": 45 }
            })),
            expected: Reported(
                Some("disabled".to_string()),
                Some(default_unreferenced),
                Some(45),
            ),
        },
        PutCase {
            label: "a status and no settings at all",
            region: "us-east-2",
            status: Some("disabled"),
            settings: None,
            expected: Reported(
                Some("disabled".to_string()),
                Some(default_unreferenced),
                Some(default_non_current),
            ),
        },
        PutCase {
            label: "settings with no status",
            region: "ca-central-1",
            status: None,
            settings: Some(json!({
                "icebergUnreferencedFileRemoval": { "nonCurrentDays": 6 }
            })),
            expected: Reported(
                // The status the put left out is the service's own.
                Some(DEFAULT_UNREFERENCED_FILE_REMOVAL.0.to_string()),
                Some(default_unreferenced),
                Some(6),
            ),
        },
    ]
}

#[tokio::test]
async fn a_new_table_bucket_is_maintained_under_the_published_defaults() {
    let emulator = Emulator::new();

    // The rule holds for every table bucket, so the rows differ in the region
    // and in the bucket's name.
    let rows = [
        ("us-east-1", "bucket-maintenance-defaults-one"),
        ("eu-west-1", "bucket-maintenance-defaults-two"),
        ("ap-northeast-1", "abc"),
    ];

    for (region, bucket) in rows {
        let case = format!("{region}/{bucket}");
        let bucket_arn = create_bucket(&emulator, region, bucket).await;

        let read = emulator
            .get_table_bucket_maintenance_configuration(region, &bucket_arn)
            .await;
        assert_eq!(
            read.text("tableBucketARN"),
            bucket_arn,
            "case {case:?}: expected the bucket's own ARN {bucket_arn:?}, got {actual:?}",
            actual = read.text("tableBucketARN")
        );

        // One entry per modeled table bucket maintenance type, each carrying the
        // published default for that task.
        let mut expected: Vec<(String, Reported)> = vec![(
            TableBucketMaintenanceType::IcebergUnreferencedFileRemoval
                .as_wire()
                .to_string(),
            Reported::default_unreferenced_file_removal(),
        )];
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        let actual = reported(&read, &case);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the published defaults {expected:?}, got {actual:?}"
        );
        assert_eq!(
            actual.len(),
            TableBucketMaintenanceType::ALL.len(),
            "case {case:?}: expected one entry per modeled task, got {actual:?}"
        );

        // A read reports what is in force without persisting a configuration,
        // so a later caller sees exactly what this one did.
        assert_eq!(
            stored_tasks(&emulator, &bucket_arn),
            Vec::<String>::new(),
            "case {case:?}: the read must persist no configuration, got {actual:?}",
            actual = stored_tasks(&emulator, &bucket_arn)
        );
        let again = emulator
            .get_table_bucket_maintenance_configuration(region, &bucket_arn)
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
    let kind = TableBucketMaintenanceType::IcebergUnreferencedFileRemoval;

    for (index, case) in put_cases().into_iter().enumerate() {
        let bucket = format!("bucket-maintenance-put-{index}");
        let bucket_arn = create_bucket(&emulator, case.region, &bucket).await;

        let put = emulator
            .put_table_bucket_maintenance_configuration(
                case.region,
                &bucket_arn,
                kind.as_wire(),
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
            stored_tasks(&emulator, &bucket_arn),
            vec![kind.as_wire().to_string()],
            "case {label:?}: the put must persist exactly the task it addressed, got {actual:?}",
            label = case.label,
            actual = stored_tasks(&emulator, &bucket_arn)
        );

        let read = emulator
            .get_table_bucket_maintenance_configuration(case.region, &bucket_arn)
            .await;
        assert_eq!(
            read.text("tableBucketARN"),
            bucket_arn,
            "case {label:?}: expected the bucket's own ARN {bucket_arn:?}, got {actual:?}",
            label = case.label,
            actual = read.text("tableBucketARN")
        );
        let actual = reported_task(&read, case.label, kind);
        assert_eq!(
            actual, case.expected,
            "case {label:?}: expected {expected:?}, got {actual:?}",
            label = case.label,
            expected = case.expected
        );

        // The put replaces the whole configuration of its task: a second put of
        // another configuration for the same task leaves no trace of the first,
        // so the settings it carries none of are the task's own defaults again.
        let replaced = emulator
            .put_table_bucket_maintenance_configuration(
                case.region,
                &bucket_arn,
                kind.as_wire(),
                &json!({ "status": "enabled" }),
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
                .get_table_bucket_maintenance_configuration(case.region, &bucket_arn)
                .await,
            case.label,
            kind,
        );
        assert_eq!(
            after,
            Reported::default_unreferenced_file_removal(),
            "case {label:?}: a put must replace the whole configuration of its task, expected \
             {expected:?}, got {after:?}",
            label = case.label,
            expected = Reported::default_unreferenced_file_removal()
        );
    }
}

#[tokio::test]
async fn each_table_bucket_carries_its_own_maintenance_configuration() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let kind = TableBucketMaintenanceType::IcebergUnreferencedFileRemoval;
    let left = create_bucket(&emulator, region, "bucket-maintenance-left").await;
    let right = create_bucket(&emulator, region, "bucket-maintenance-right").await;

    let put = emulator
        .put_table_bucket_maintenance_configuration(
            region,
            &left,
            kind.as_wire(),
            &json!({ "status": "disabled", "settings": {
                "icebergUnreferencedFileRemoval": { "unreferencedDays": 2, "nonCurrentDays": 4 }
            }}),
        )
        .await;
    assert_eq!(
        put.status, 204,
        "configuring the left bucket must succeed, got {status} {body}",
        status = put.status,
        body = put.body
    );

    for (case, arn, expected) in [
        (
            "the bucket the put addressed",
            &left,
            Reported(Some("disabled".to_string()), Some(2), Some(4)),
        ),
        (
            "its sibling, which no put touched",
            &right,
            Reported::default_unreferenced_file_removal(),
        ),
    ] {
        let read = emulator
            .get_table_bucket_maintenance_configuration(region, arn)
            .await;
        let actual = reported_task(&read, case, kind);
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
    let kind = TableBucketMaintenanceType::IcebergUnreferencedFileRemoval;
    let bucket_arn = create_bucket(&emulator, region, "bucket-maintenance-refusals").await;

    // `PositiveInteger` is `@range(1, 2147483647)`, so each settings member is
    // refused one step past its bounds, and a status outside the modeled enum is
    // refused too. Every row here is refused by the constraint boundary before
    // the handler runs, so they are graded on the status and on nothing being
    // persisted — which code and text a constraint violation carries is one rule
    // for every operation of this service and belongs to the suite that covers
    // it (`constraint_refusals.rs`).
    let refused: Vec<(&str, Value)> = vec![
        (
            "an unreferenced-days below the modeled minimum",
            json!({ "status": "enabled", "settings": {
                "icebergUnreferencedFileRemoval": { "unreferencedDays": 0 }
            }}),
        ),
        (
            "a negative unreferenced-days",
            json!({ "status": "enabled", "settings": {
                "icebergUnreferencedFileRemoval": { "unreferencedDays": -1 }
            }}),
        ),
        (
            "a noncurrent-days below the modeled minimum",
            json!({ "status": "enabled", "settings": {
                "icebergUnreferencedFileRemoval": { "nonCurrentDays": 0 }
            }}),
        ),
        (
            "a noncurrent-days past the modeled maximum",
            json!({ "status": "enabled", "settings": {
                "icebergUnreferencedFileRemoval": { "nonCurrentDays": 2147483648i64 }
            }}),
        ),
        (
            "a status the model does not declare",
            json!({ "status": "paused" }),
        ),
    ];

    for (case, value) in refused {
        let reply = emulator
            .put_table_bucket_maintenance_configuration(region, &bucket_arn, kind.as_wire(), &value)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected the client-input refusal 400, got {status} {body}",
            status = reply.status,
            body = reply.body
        );
        assert_eq!(
            stored_tasks(&emulator, &bucket_arn),
            Vec::<String>::new(),
            "case {case:?}: a refused put must persist nothing, got {actual:?}",
            actual = stored_tasks(&emulator, &bucket_arn)
        );
    }

    // And the bucket still reads back under the defaults, so the refusals above
    // changed nothing about it.
    let read = emulator
        .get_table_bucket_maintenance_configuration(region, &bucket_arn)
        .await;
    assert_eq!(
        reported_task(&read, "after the refusals", kind),
        Reported::default_unreferenced_file_removal(),
        "after the refusals: the bucket must still run under the published default"
    );
}

#[tokio::test]
async fn a_call_against_a_table_bucket_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let kind = TableBucketMaintenanceType::IcebergUnreferencedFileRemoval;
    let bucket_arn = create_bucket(&emulator, region, "bucket-maintenance-present").await;

    let cases: Vec<(&str, String)> = vec![
        (
            "a table bucket that was never created",
            "arn:aws:s3tables:us-east-1:000000000000:bucket/bucket-maintenance-absent".to_string(),
        ),
        (
            "the resident bucket's name in another account",
            bucket_arn.replace("000000000000", "123456789012"),
        ),
        (
            "the resident bucket's name in another region",
            bucket_arn.replace("us-east-1", "eu-west-1"),
        ),
    ];

    for (case, arn) in cases {
        for (operation, reply) in [
            (
                "GetTableBucketMaintenanceConfiguration",
                emulator
                    .get_table_bucket_maintenance_configuration(region, &arn)
                    .await,
            ),
            (
                "PutTableBucketMaintenanceConfiguration",
                emulator
                    .put_table_bucket_maintenance_configuration(
                        region,
                        &arn,
                        kind.as_wire(),
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
                NO_BUCKET,
                "case {case:?} on {operation}: expected {NO_BUCKET:?}, got {actual:?}",
                actual = reply.message()
            );
        }
    }

    // The resident bucket still answers, so the refusals above are about what
    // each named and not about an empty store.
    let read = emulator
        .get_table_bucket_maintenance_configuration(region, &bucket_arn)
        .await;
    assert_eq!(
        read.status, 200,
        "the resident bucket must still answer, got {status} {body}",
        status = read.status,
        body = read.body
    );
}
