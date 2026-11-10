// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRDSDatabaseRecommendations`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same `MultiServiceRouter` the emulator binary mounts — so a response that is
//! not computed from persisted state fails here.
//!
//! An Amazon Aurora or RDS database recommendation is produced by Compute Optimizer's own
//! analysis of an observed workload, and no operation of THIS API creates one, so the
//! fixtures below record rows into the findings catalog the way that analysis would and then
//! read them back only over the wire. What is asserted is the RULE the read follows — report
//! one recommendation per analysed database row, each member computed from the stored row and
//! from the preferences in force for it, selected by resource, account and filter, paged at
//! `maxResults`, with a requested resource that has no database row absent from the answer —
//! over a table of inputs, never one recorded call.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::{
    DBStorageConfiguration, RDSDBUtilizationMetric, Tag,
};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    service_state, AnalysedResource, RdsDatabaseDetail, ANALYSIS_DELAY, DEFAULT_LOOK_BACK_PERIOD,
    MAX_PAGE_SIZE, SOURCE_TYPE_RDS_DB_INSTANCE,
};

use crate::harness::{
    call, call_in_region, emulator_with_engine, error_code, instance_arn, opt_in,
};

/// Another account entirely, which this service cannot be asked about: `accountIds` is how
/// an organization's MANAGEMENT account names a member account, and this emulator presents
/// one standalone account and records no organization. Used below for the values that must
/// be REFUSED, and for the catalog row that must be reported to nobody.
const MEMBER_ACCOUNT: &str = "210987654321";

/// The region the harness signs for, and so the regional endpoint every call below
/// addresses unless it names another.
const REGION: &str = "us-east-1";

/// The ONE account this service answers for, taken from the engine rather than written out
/// here: it owns every analysed database this service can report, including the one its own
/// catalog is seeded with.
fn own_account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The `lastRefreshTimestamp` the recorded rows carry, in epoch seconds — the form awsJson1_0
/// renders a timestamp in.
const REFRESHED_AT_SECONDS: i64 = 1_761_000_000;

/// An ARN in the grammar every AWS service shares:
/// `arn:<partition>:<service>:<region>:<account>:<resource>`.
fn arn(service: &str, account_id: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{REGION}:{account_id}:{resource}")
}

/// A DB instance ARN: "arn:aws:rds:{region}:{accountId}:db:{resourceName}"
/// (`GetRDSDatabaseRecommendations$resourceArns`).
fn db_arn(account_id: &str, name: &str) -> String {
    arn("rds", account_id, &format!("db:{name}"))
}

/// A DB cluster ARN: "arn:aws:rds:{region}:{accountId}:cluster:{resourceName}" (same doc).
fn cluster_arn(account_id: &str, name: &str) -> String {
    arn("rds", account_id, &format!("cluster:{name}"))
}

/// One analysed resource as the findings catalog records it, for a resource analysed as an
/// Amazon Aurora or RDS database. The catalog records the resource's region-free identity:
/// the ARN reported for it names the region of the endpoint that reports it.
#[allow(clippy::too_many_arguments)]
fn analysed_database(
    account_id: &str,
    arn_resource: &str,
    finding: &str,
    finding_reason_codes: &[&str],
    current_performance_risk: &str,
    current_monthly_cost: f64,
    recommended_monthly_cost: f64,
    rds_database: Option<RdsDatabaseDetail>,
) -> AnalysedResource {
    AnalysedResource {
        account_id: account_id.to_string(),
        resource_type: SOURCE_TYPE_RDS_DB_INSTANCE.to_string(),
        arn_service: "rds".to_string(),
        arn_resource: arn_resource.to_string(),
        finding: finding.to_string(),
        finding_reason_codes: finding_reason_codes
            .iter()
            .map(|code| (*code).to_string())
            .collect(),
        current_performance_risk: current_performance_risk.to_string(),
        current_monthly_cost,
        recommended_monthly_cost,
        inferred_workload_types: Vec::new(),
        rds_database,
    }
}

/// A storage configuration (`DBStorageConfiguration`); `storageType` is a free-form string
/// in the model, so these are the EBS volume type names RDS reports.
fn storage(storage_type: &str, allocated: i32, iops: i32) -> DBStorageConfiguration {
    DBStorageConfiguration {
        storage_type: Some(storage_type.to_string()),
        allocated_storage: allocated,
        iops: Some(iops),
        max_allocated_storage: Some(allocated * 2),
        storage_throughput: Some(125),
    }
}

/// The first analysed database every fixture below records: an over-provisioned PostgreSQL
/// instance the analysis would move to the next class down. The row
/// [`an_analysed_database_is_reported_from_its_catalog_row`] reads member for member, and the
/// first row of the catalog the selection and paging fixtures walk.
fn primary_database() -> AnalysedResource {
    analysed_database(
        own_account(),
        "db:orders-primary",
        "Overprovisioned",
        &[],
        "Medium",
        300.0,
        225.0,
        Some(RdsDatabaseDetail {
            // `Engine`/`EngineVersion` are free-form strings in the model; these are the
            // engine name and version an RDS PostgreSQL instance reports.
            engine: Some("postgres".to_string()),
            engine_version: Some("15.4".to_string()),
            // Not a cluster member, so no promotion tier and no cluster identifier.
            promotion_tier: None,
            db_cluster_identifier: None,
            current_db_instance_class: Some("db.r6g.2xlarge".to_string()),
            recommended_db_instance_class: Some("db.r6g.xlarge".to_string()),
            current_storage_configuration: Some(DBStorageConfiguration {
                storage_type: Some("gp3".to_string()),
                allocated_storage: 500,
                iops: Some(3_000),
                max_allocated_storage: Some(1_000),
                storage_throughput: Some(125),
            }),
            // The analysis would keep the storage as it is: its finding is about the
            // instance, so there is no storage configuration to move to.
            recommended_storage_configuration: None,
            // `RDSDBMetricName` / `RDSDBMetricStatistic` values.
            utilization_metrics: vec![
                RDSDBUtilizationMetric {
                    name: Some("CPU".to_string()),
                    statistic: Some("Maximum".to_string()),
                    value: 17.5,
                },
                RDSDBUtilizationMetric {
                    name: Some("Memory".to_string()),
                    statistic: Some("Maximum".to_string()),
                    value: 24.0,
                },
                RDSDBUtilizationMetric {
                    name: Some("DatabaseConnections".to_string()),
                    statistic: Some("Maximum".to_string()),
                    value: 12.0,
                },
            ],
            last_refresh_timestamp: None,
            tags: Vec::new(),
        }),
    )
}

/// The analysis output every fixture below reads beside [`primary_database`]: five analysed
/// databases of the account this service answers for, covering every `RDSInstanceFinding` plus
/// the one finding only `RDSStorageFinding` declares, recommendations that save / grow /
/// change nothing, and a row whose database configuration the analysis did not record.
fn corpus() -> Vec<AnalysedResource> {
    vec![
        // Over-provisioned: the recommendation is a smaller DB instance class, so it saves.
        // One reason code of `RDSInstanceFindingReasonCode` and one of neither RDS
        // vocabulary (`MemoryOverprovisioned` is a `FindingReasonCode`), so the partition
        // rule has something to drop.
        analysed_database(
            own_account(),
            "db:orders-writer",
            "Overprovisioned",
            &["CPUOverprovisioned", "MemoryOverprovisioned"],
            "Low",
            200.0,
            150.0,
            Some(RdsDatabaseDetail {
                engine: Some("aurora-mysql".to_string()),
                engine_version: Some("3.05.2".to_string()),
                promotion_tier: Some(1),
                db_cluster_identifier: Some("orders-cluster".to_string()),
                current_db_instance_class: Some("db.r6g.4xlarge".to_string()),
                recommended_db_instance_class: Some("db.r6g.2xlarge".to_string()),
                current_storage_configuration: Some(storage("gp3", 400, 3_000)),
                recommended_storage_configuration: None,
                utilization_metrics: vec![RDSDBUtilizationMetric {
                    // `RDSDBMetricName` / `RDSDBMetricStatistic` values.
                    name: Some("CPU".to_string()),
                    statistic: Some("Maximum".to_string()),
                    value: 12.0,
                }],
                last_refresh_timestamp: Some(::aws_smithy_types::DateTime::from_secs(
                    REFRESHED_AT_SECONDS,
                )),
                tags: vec![Tag {
                    key: Some("Owner".to_string()),
                    value: Some("TeamDb".to_string()),
                }],
            }),
        ),
        // Under-provisioned: the recommendation is LARGER, so the configuration changes but
        // nothing is saved.
        analysed_database(
            own_account(),
            "db:reports-primary",
            "Underprovisioned",
            &["MemoryUnderprovisioned"],
            "High",
            100.0,
            180.0,
            Some(RdsDatabaseDetail {
                engine: Some("postgres".to_string()),
                engine_version: Some("14.9".to_string()),
                current_db_instance_class: Some("db.t4g.medium".to_string()),
                recommended_db_instance_class: Some("db.t4g.large".to_string()),
                ..RdsDatabaseDetail::default()
            }),
        ),
        // Optimized: nothing to change, so there is no recommendation option at all — and
        // the analysis recorded no database configuration for this row.
        analysed_database(
            own_account(),
            "db:audit-archive",
            "Optimized",
            &[],
            "VeryLow",
            90.0,
            90.0,
            None,
        ),
        // The one finding only `RDSStorageFinding` declares: it is the STORAGE that is not
        // optimized, so the finding and the option belong to the storage members.
        analysed_database(
            own_account(),
            "cluster:ledger-cluster",
            "NotOptimized",
            &[
                // `RDSStorageFindingReasonCode` values.
                "EBSVolumeIOPSOverprovisioned",
                "NewGenerationStorageTypeAvailable",
            ],
            "Medium",
            60.0,
            40.0,
            Some(RdsDatabaseDetail {
                engine: Some("mysql".to_string()),
                engine_version: Some("8.0.35".to_string()),
                current_db_instance_class: Some("db.m6g.large".to_string()),
                recommended_db_instance_class: None,
                current_storage_configuration: Some(storage("io1", 200, 5_000)),
                recommended_storage_configuration: Some(storage("gp3", 200, 3_000)),
                ..RdsDatabaseDetail::default()
            }),
        ),
        // A fifth database of the same account, recorded with no reason code at all.
        analysed_database(
            own_account(),
            "db:billing-primary",
            "Overprovisioned",
            &[],
            "VeryLow",
            50.0,
            25.0,
            Some(RdsDatabaseDetail {
                engine: Some("mariadb".to_string()),
                current_db_instance_class: Some("db.m5.large".to_string()),
                recommended_db_instance_class: Some("db.m5.xlarge".to_string()),
                ..RdsDatabaseDetail::default()
            }),
        ),
    ]
}

/// Record `resources` into the findings catalog this reader reports from, the way Compute
/// Optimizer's analysis would.
fn record(engine: &Arc<engine::InMemoryEngine>, resources: &[AnalysedResource]) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for resource in resources {
        guard.analysed_resources.push(resource.clone());
    }
}

/// How many rows the findings catalog holds.
fn catalog_size(engine: &Arc<engine::InMemoryEngine>) -> usize {
    service_state(&**engine)
        .expect("the engine bears this service's state")
        .lock()
        .analysed_resources
        .len()
}

/// The `resourceArn` of every recommendation in a response, in the order reported.
fn reported_arns(body: &Value) -> Vec<String> {
    body["rdsDBRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of rdsDBRecommendations, got {body}"))
        .iter()
        .map(|recommendation| {
            recommendation["resourceArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// An emulator whose account is opted in, whose analysis is due and whose findings catalog
/// holds [`primary_database`] and then [`corpus`].
///
/// A recommendation is reported only once the analysis the enrollment started is due
/// ([`ANALYSIS_DELAY`]), so the fixture waits that span out before reading, the way every
/// other recommendation suite of this crate does; the extra margin covers scheduler jitter.
async fn emulator_with_corpus() -> (
    smithy_server_runtime::MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
    Vec<AnalysedResource>,
) {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    let corpus = corpus();
    record(&engine, &[primary_database()]);
    record(&engine, &corpus);
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    (router, engine, corpus)
}

/// Read, asserting the call was accepted.
async fn get_rds(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: Value,
) -> Value {
    let (status, body) = call(router, "GetRDSDatabaseRecommendations", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {request}, got {status}: {body}"
    );
    body
}

/// The recommendation reported for one ARN.
async fn reported_for(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> Value {
    let body = get_rds(router, case, json!({"resourceArns": [resource_arn]})).await;
    let reported = body["rdsDBRecommendations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {body}"));
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly the recommendation of {resource_arn}, got {body}"
    );
    reported[0].clone()
}

/// Assert every member of `expected` is reported as given.
fn assert_members(case: &str, reported: &Value, expected: &Value) {
    for (member, wanted) in expected.as_object().expect("an object of members") {
        assert_eq!(
            &reported[member], wanted,
            "case {case:?}: member {member:?}: expected {wanted}, got {reported}"
        );
    }
}

/// An analysed database the findings catalog holds is reported from that row, member for
/// member, and the savings it reports are the ones `GetRecommendationSummaries` aggregates
/// for the same resource type — one catalog seen two ways. A read reports: it must not change
/// what a later caller sees.
#[tokio::test]
async fn an_analysed_database_is_reported_from_its_catalog_row() {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    record(&engine, &[primary_database()]);
    // The catalog is read only once the analysis the enrollment started is due.
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
    let seeded = db_arn(own_account(), "orders-primary");
    let before = catalog_size(&engine);

    let body = get_rds(&router, "the whole catalog", json!({})).await;
    assert_eq!(
        reported_arns(&body),
        vec![seeded.clone()],
        "the catalog holds exactly one analysed database, got {body}"
    );
    let reported = &body["rdsDBRecommendations"][0];
    assert_members(
        "the seeded database",
        reported,
        // Stored: Overprovisioned, Medium risk, 300.00/month against a recommended 225.00.
        &json!({
            "resourceArn": seeded,
            "accountId": own_account(),
            "engine": "postgres",
            "engineVersion": "15.4",
            "currentDBInstanceClass": "db.r6g.2xlarge",
            "currentStorageConfiguration": {
                "storageType": "gp3",
                "allocatedStorage": 500,
                "iops": 3000,
                "maxAllocatedStorage": 1000,
                "storageThroughput": 125,
            },
            "idle": "False",
            "instanceFinding": "Overprovisioned",
            // Nothing was recorded against the storage, so the analysis would not change it.
            "storageFinding": "Optimized",
            "currentInstancePerformanceRisk": "Medium",
            "instanceRecommendationOptions": [{
                "dbInstanceClass": "db.r6g.xlarge",
                "rank": 1,
                // The same work on half the capacity: CPU and memory are shares of the
                // class's capacity, so each doubles from db.r6g.2xlarge to db.r6g.xlarge,
                // while the connection count the workload makes does not move.
                "projectedUtilizationMetrics": [
                    {"name": "CPU", "statistic": "Maximum", "value": 35.0},
                    {"name": "Memory", "statistic": "Maximum", "value": 48.0},
                    {"name": "DatabaseConnections", "statistic": "Maximum", "value": 12.0},
                ],
                // The most loaded share takes 48% of the recommended class, which is
                // (0.48 - 0.25) / 0.25 on the modelled 0..4 risk scale.
                "performanceRisk": 0.92,
                "savingsOpportunity": {
                    "savingsOpportunityPercentage": 25.0,
                    "estimatedMonthlySavings": {"currency": "USD", "value": 75.0},
                },
            }],
            "utilizationMetrics": [
                {"name": "CPU", "statistic": "Maximum", "value": 17.5},
                {"name": "Memory", "statistic": "Maximum", "value": 24.0},
                {"name": "DatabaseConnections", "statistic": "Maximum", "value": 12.0},
            ],
            "effectiveRecommendationPreferences": {
                // The status in force with no preference on record: the preference's
                // vocabulary names the not-applied state itself.
                "enhancedInfrastructureMetrics": "Inactive",
                "lookBackPeriod": DEFAULT_LOOK_BACK_PERIOD,
                // Every money figure is computed from the prices stored with this
                // emulator's own analysis.
                "savingsEstimationMode": {"source": "PublicPricing"},
            },
            "lookbackPeriodInDays": 14.0,
        }),
    );
    // Nothing was recorded for these, so they are omitted rather than invented.
    for absent in [
        "storageRecommendationOptions",
        "promotionTier",
        "dbClusterIdentifier",
        "lastRefreshTimestamp",
        "tags",
    ] {
        assert!(
            reported[absent].is_null(),
            "member {absent:?}: the analysis recorded none, so none is reported, got {reported}"
        );
    }
    assert_eq!(
        body["errors"],
        json!([]),
        "no resource was named, so nothing is missing a recommendation, got {body}"
    );

    // The same rows, aggregated: the savings this reader reports per database must be the
    // savings the summary reports for the resource type.
    let (status, summaries) = call(&router, "GetRecommendationSummaries", json!({})).await;
    assert_eq!(status, 200, "summaries: expected 200, got {status}: {summaries}");
    let summary = summaries["recommendationSummaries"]
        .as_array()
        .expect("a list of summaries")
        .iter()
        .find(|summary| summary["recommendationResourceType"] == json!(SOURCE_TYPE_RDS_DB_INSTANCE))
        .unwrap_or_else(|| panic!("a summary for {SOURCE_TYPE_RDS_DB_INSTANCE}: {summaries}"))
        .clone();
    assert_eq!(
        summary["savingsOpportunity"]["estimatedMonthlySavings"]["value"],
        reported["instanceRecommendationOptions"][0]["savingsOpportunity"]
            ["estimatedMonthlySavings"]["value"],
        "the per-database saving and the summarised saving come from the same catalog row: \
         {summary} vs {reported}"
    );

    let again = get_rds(&router, "the same read again", json!({})).await;
    assert_eq!(
        again, body,
        "a second read must report the same recommendation, got {again}"
    );
    assert_eq!(
        catalog_size(&engine),
        before,
        "a read must not add or drop a catalog row"
    );
}

/// Both findings are reported on every recommendation, each in the vocabulary its own member
/// declares: the finding the catalog recorded goes to the side whose enum declares it — the
/// instance side for the three values both declare, the storage side for the one only
/// `RDSStorageFinding` declares — and the other side reads `Optimized`, the value both use
/// for a configuration the analysis would not change. The recommendation option goes on the
/// side that is not optimized, and an analysis that would change nothing reports no option.
#[tokio::test]
async fn both_findings_are_reported_each_in_its_own_vocabulary() {
    let (router, _engine, corpus) = emulator_with_corpus().await;

    // (case, the database, instanceFinding, storageFinding, the option member expected)
    let rows: Vec<(&str, &AnalysedResource, Value, Value, Option<&str>)> = vec![
        (
            "an over-provisioned instance saves",
            &corpus[0],
            json!("Overprovisioned"),
            json!("Optimized"),
            Some("instanceRecommendationOptions"),
        ),
        (
            "an under-provisioned instance changes but saves nothing",
            &corpus[1],
            json!("Underprovisioned"),
            json!("Optimized"),
            Some("instanceRecommendationOptions"),
        ),
        (
            "an optimized database has nothing to recommend on either side",
            &corpus[2],
            json!("Optimized"),
            json!("Optimized"),
            None,
        ),
        (
            "a not-optimized storage configuration is a storage finding",
            &corpus[3],
            json!("Optimized"),
            json!("NotOptimized"),
            Some("storageRecommendationOptions"),
        ),
    ];

    for (case, resource, instance_finding, storage_finding, option_member) in rows {
        let reported = reported_for(&router, case, &resource.arn_in(REGION)).await;
        assert_eq!(
            reported["instanceFinding"], instance_finding,
            "case {case:?}: expected instanceFinding {instance_finding}, got {reported}"
        );
        assert_eq!(
            reported["storageFinding"], storage_finding,
            "case {case:?}: expected storageFinding {storage_finding}, got {reported}"
        );
        for member in ["instanceRecommendationOptions", "storageRecommendationOptions"] {
            if option_member == Some(member) {
                // The saving is what the stored costs say it is, and nothing the
                // recommendation would GROW saves money.
                let savings = (resource.current_monthly_cost - resource.recommended_monthly_cost)
                    .max(0.0);
                let percentage = (savings / resource.current_monthly_cost * 10_000.0).round()
                    / 100.0;
                assert_eq!(
                    reported[member][0]["savingsOpportunity"],
                    json!({
                        "savingsOpportunityPercentage": percentage,
                        "estimatedMonthlySavings": {"currency": "USD", "value": savings},
                    }),
                    "case {case:?}: {member} must carry the saving the stored costs imply, \
                     got {reported}"
                );
                assert_eq!(
                    reported[member][0]["rank"],
                    json!(1),
                    "case {case:?}: the one recorded option ranks first, got {reported}"
                );
            } else {
                assert!(
                    reported[member].is_null(),
                    "case {case:?}: member {member:?} must be absent, got {reported}"
                );
            }
        }
    }
}

/// A stored reason code is reported on the member whose enum declares it, and on no other: a
/// code outside both RDS vocabularies is reported on neither.
#[tokio::test]
async fn reason_codes_are_reported_on_the_member_that_can_carry_them() {
    let (router, _engine, corpus) = emulator_with_corpus().await;

    // (case, the database, instanceFindingReasonCodes, storageFindingReasonCodes)
    let rows: Vec<(&str, &AnalysedResource, Value, Value)> = vec![
        (
            "an instance code is kept and a code of neither vocabulary dropped",
            &corpus[0],
            json!(["CPUOverprovisioned"]),
            Value::Null,
        ),
        (
            "an instance code the model declares only for RDS instances",
            &corpus[1],
            json!(["MemoryUnderprovisioned"]),
            Value::Null,
        ),
        (
            "no stored code is no reported code",
            &corpus[2],
            Value::Null,
            Value::Null,
        ),
        (
            "two storage codes, in stored order",
            &corpus[3],
            Value::Null,
            json!([
                "EBSVolumeIOPSOverprovisioned",
                "NewGenerationStorageTypeAvailable"
            ]),
        ),
    ];

    for (case, resource, instance_codes, storage_codes) in rows {
        let reported = reported_for(&router, case, &resource.arn_in(REGION)).await;
        assert_eq!(
            reported["instanceFindingReasonCodes"], instance_codes,
            "case {case:?}: expected {instance_codes}, got {reported}"
        );
        assert_eq!(
            reported["storageFindingReasonCodes"], storage_codes,
            "case {case:?}: expected {storage_codes}, got {reported}"
        );
    }
}

/// The selection rule: a request reports the analysed databases of the resources it names,
/// owned by the one account this service answers for, whose recommendation satisfies every
/// filter it carries — and a member it omits selects nothing away. Each row differs along one
/// of those dimensions.
#[tokio::test]
async fn the_read_selects_by_resource_account_and_filter() {
    let (router, _engine, corpus) = emulator_with_corpus().await;
    let [writer, reports, archive, ledger, billing] =
        [0, 1, 2, 3, 4].map(|index| corpus[index].arn_in(REGION));
    let seeded = db_arn(own_account(), "orders-primary");

    // (case, request, the ARNs expected in the order expected)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "no selection reports every analysed database, in catalog order",
            json!({}),
            vec![
                seeded.clone(),
                writer.clone(),
                reports.clone(),
                archive.clone(),
                ledger.clone(),
                billing.clone(),
            ],
        ),
        (
            "one resource ARN",
            json!({"resourceArns": [ledger.clone()]}),
            vec![ledger.clone()],
        ),
        (
            "three resource ARNs, reported in catalog order",
            json!({"resourceArns": [billing.clone(), writer.clone(), archive.clone()]}),
            vec![writer.clone(), archive.clone(), billing.clone()],
        ),
        (
            "the caller's own account, named explicitly, selects nothing away",
            json!({"accountIds": [own_account()]}),
            vec![
                seeded.clone(),
                writer.clone(),
                reports.clone(),
                archive.clone(),
                ledger.clone(),
                billing.clone(),
            ],
        ),
        (
            "an empty accountIds list names no account and so selects nothing away",
            json!({"accountIds": []}),
            vec![
                seeded.clone(),
                writer.clone(),
                reports.clone(),
                archive.clone(),
                ledger.clone(),
                billing.clone(),
            ],
        ),
        (
            "one instance finding",
            json!({"filters": [{"name": "InstanceFinding", "values": ["Underprovisioned"]}]}),
            vec![reports.clone()],
        ),
        (
            "two instance findings",
            json!({"filters": [
                {"name": "InstanceFinding", "values": ["Optimized", "Overprovisioned"]},
            ]}),
            vec![
                seeded.clone(),
                writer.clone(),
                archive.clone(),
                ledger.clone(),
                billing.clone(),
            ],
        ),
        (
            "a storage finding",
            json!({"filters": [{"name": "StorageFinding", "values": ["NotOptimized"]}]}),
            vec![ledger.clone()],
        ),
        (
            "the storage of every other analysed database is optimized",
            json!({"filters": [{"name": "StorageFinding", "values": ["Optimized"]}]}),
            vec![
                seeded.clone(),
                writer.clone(),
                reports.clone(),
                archive.clone(),
                billing.clone(),
            ],
        ),
        (
            "an instance finding reason code",
            json!({"filters": [
                {"name": "InstanceFindingReasonCode", "values": ["MemoryUnderprovisioned"]},
            ]}),
            vec![reports.clone()],
        ),
        (
            "a storage finding reason code",
            json!({"filters": [
                {"name": "StorageFindingReasonCode", "values": ["NewGenerationStorageTypeAvailable"]},
            ]}),
            vec![ledger.clone()],
        ),
        (
            "a reason code no analysed database carries",
            json!({"filters": [
                {"name": "InstanceFindingReasonCode", "values": ["NewEngineVersionAvailable"]},
            ]}),
            Vec::new(),
        ),
        (
            "no analysed database is idle",
            json!({"filters": [{"name": "Idle", "values": ["True"]}]}),
            Vec::new(),
        ),
        (
            "every analysed database is not idle",
            json!({"filters": [{"name": "Idle", "values": ["False"]}]}),
            vec![
                seeded.clone(),
                writer.clone(),
                reports.clone(),
                archive.clone(),
                ledger.clone(),
                billing.clone(),
            ],
        ),
        (
            "two filters are ANDed",
            json!({"filters": [
                {"name": "InstanceFinding", "values": ["Overprovisioned"]},
                {"name": "InstanceFindingReasonCode", "values": ["CPUOverprovisioned"]},
            ]}),
            vec![writer.clone()],
        ),
        (
            "a filter carrying no values constrains nothing",
            json!({"filters": [{"name": "InstanceFinding", "values": []}]}),
            vec![
                seeded.clone(),
                writer.clone(),
                reports.clone(),
                archive.clone(),
                ledger.clone(),
                billing.clone(),
            ],
        ),
        (
            "the caller's own account and two resource ARNs are ANDed",
            json!({
                "accountIds": [own_account()],
                "resourceArns": [writer.clone(), billing.clone()],
            }),
            vec![writer.clone(), billing.clone()],
        ),
        (
            "an account and a filter are ANDed",
            json!({
                "accountIds": [own_account()],
                "filters": [{"name": "StorageFinding", "values": ["NotOptimized"]}],
            }),
            vec![ledger.clone()],
        ),
    ];

    for (case, request, wanted) in rows {
        let body = get_rds(&router, case, request).await;
        assert_eq!(
            reported_arns(&body),
            wanted,
            "case {case:?}: expected {wanted:?}, got {body}"
        );
    }
}

/// `accountIds` is the mechanism by which an organization's MANAGEMENT account names a
/// MEMBER account, and this emulator presents one standalone account with no organization:
/// a well-formed 12-digit id naming any other account asks for recommendations this service
/// cannot report, so it is refused with `InvalidParameterValueException` (400) rather than
/// used as a key to select catalog rows with. An analysed database another account owns is
/// therefore reported to nobody — not to a read that names no account, not to one that names
/// the caller's own, not to one whose filter its finding would match, and not to one that
/// names its very ARN, which is answered as a resource this service holds no recommendation
/// for.
#[tokio::test]
async fn another_account_is_refused_and_its_analysed_database_is_reported_to_nobody() {
    let (router, engine, corpus) = emulator_with_corpus().await;

    // What the catalog holds for the account this service answers for: the primary database
    // plus the recorded corpus, in catalog order.
    let mut own_databases = vec![db_arn(own_account(), "orders-primary")];
    own_databases.extend(corpus.iter().map(|resource| resource.arn_in(REGION)));

    // One more analysed database, owned by an account this service cannot be asked about —
    // the state the catalog is in whenever an analysis recorded a member account's resource.
    let foreign = analysed_database(
        MEMBER_ACCOUNT,
        "db:members-primary",
        "Overprovisioned",
        &["CPUOverprovisioned"],
        "Low",
        80.0,
        40.0,
        Some(RdsDatabaseDetail {
            engine: Some("mysql".to_string()),
            current_db_instance_class: Some("db.m6g.xlarge".to_string()),
            recommended_db_instance_class: Some("db.m6g.large".to_string()),
            ..RdsDatabaseDetail::default()
        }),
    );
    record(&engine, std::slice::from_ref(&foreign));
    let foreign_arn = foreign.arn_in(REGION);

    // Every read that could reach that row reports it to nobody, and each still reports the
    // caller's own databases. The last row also pins what a read that names that database's
    // very ARN answers: nothing reported, and no error — exactly as an ARN nothing analysed
    // is answered.
    let reads: Vec<(&str, Value, Vec<String>)> = vec![
        ("no selection", json!({}), own_databases.clone()),
        (
            "the caller's own account",
            json!({"accountIds": [own_account()]}),
            own_databases.clone(),
        ),
        (
            "an empty account list",
            json!({"accountIds": []}),
            own_databases.clone(),
        ),
        (
            "the filter the foreign row's own finding would match",
            json!({"filters": [{"name": "InstanceFinding", "values": ["Overprovisioned"]}]}),
            vec![
                own_databases[0].clone(),
                own_databases[1].clone(),
                own_databases[5].clone(),
            ],
        ),
        (
            "the foreign database's own ARN",
            json!({"resourceArns": [foreign_arn.clone()]}),
            Vec::new(),
        ),
    ];
    for (case, request, wanted) in reads {
        let body = get_rds(&router, case, request).await;
        assert_eq!(
            reported_arns(&body),
            wanted,
            "case {case:?}: expected {wanted:?} and never {foreign_arn}, got {body}"
        );
        assert_eq!(
            body["errors"],
            json!([]),
            "case {case:?}: expected no errors entry, got {body}"
        );
    }

    // Well-formed ids that are not the caller's: the account the row above belongs to, the
    // documentation's own example account, an id one digit from the caller's own, and the
    // extremes of the 12-digit range. Each is refused whatever else the request carries.
    // (The wording of the refusal is pinned for every reader in `cross_account_requests`.)
    let other_accounts = [
        MEMBER_ACCOUNT,
        "111122223333",
        "123456789012",
        "000000000001",
        "999999999999",
    ];
    for account_id in other_accounts {
        for (case, request) in [
            ("alone", json!({"accountIds": [account_id]})),
            (
                "with that account's own database named",
                json!({"accountIds": [account_id], "resourceArns": [foreign_arn.clone()]}),
            ),
            (
                "with a database of the caller's own account named",
                json!({
                    "accountIds": [account_id],
                    "resourceArns": [own_databases[0].clone()],
                }),
            ),
            (
                "with a filter",
                json!({
                    "accountIds": [account_id],
                    "filters": [{"name": "Idle", "values": ["False"]}],
                }),
            ),
            (
                "with a page size",
                json!({"accountIds": [account_id], "maxResults": 1}),
            ),
        ] {
            let (status, body) =
                call(&router, "GetRDSDatabaseRecommendations", request.clone()).await;
            assert_eq!(
                status, 400,
                "case {account_id}/{case:?}: {request} must be refused, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {account_id}/{case:?}: expected InvalidParameterValueException, got {body}"
            );
        }
    }
}

/// `idle` says whether this service also holds an IDLE recommendation for the database — the
/// record `GetIdleRecommendations` reports — and the `Idle` filter selects on it.
#[tokio::test]
async fn idle_is_true_exactly_where_an_idle_recommendation_is_held() {
    use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::IdleResourceAnalysis;

    let (router, engine, corpus) = emulator_with_corpus().await;
    let idle_row = corpus[1].clone();
    let idle_database = idle_row.arn_in(REGION);
    {
        let state = service_state(&*engine).expect("the engine bears this service's state");
        // Recorded the way Compute Optimizer's own idle analysis would: the resource's
        // region-free identity, the finding it reached, and what the database costs.
        state
            .lock()
            .idle_recommendations
            .insert(IdleResourceAnalysis {
                account_id: idle_row.account_id.clone(),
                arn_service: idle_row.arn_service.clone(),
                arn_resource: idle_row.arn_resource.clone(),
                finding: "Idle".to_string(),
                utilization_metrics: Vec::new(),
                days_observed: 14.0,
                monthly_cost: idle_row.current_monthly_cost,
                tags: Vec::new(),
            });
    }

    for resource in &corpus {
        let arn = resource.arn_in(REGION);
        let reported = reported_for(&router, &arn, &arn).await;
        let wanted = if arn == idle_database { "True" } else { "False" };
        assert_eq!(
            reported["idle"],
            json!(wanted),
            "database {arn}: expected idle {wanted}, got {reported}"
        );
    }

    let body = get_rds(
        &router,
        "the idle filter",
        json!({"filters": [{"name": "Idle", "values": ["True"]}]}),
    )
    .await;
    assert_eq!(
        reported_arns(&body),
        vec![idle_database.clone()],
        "only the database with an idle recommendation is idle, got {body}"
    );
}

/// The look-back period in force sets both `effectiveRecommendationPreferences.lookBackPeriod`
/// and `lookbackPeriodInDays`, and the most specific scope that sets it wins. Every value the
/// model's `LookBackPeriodPreference` declares is covered.
#[tokio::test]
async fn the_look_back_preference_in_force_sets_the_reported_days() {
    let (router, _engine, corpus) = emulator_with_corpus().await;
    let database = corpus[0].arn_in(REGION);

    // (case, the preference to write first, the preference expected, the days expected)
    let rows: Vec<(&str, Option<Value>, &str, f64)> = vec![
        (
            "no preference uses the documented default",
            None,
            DEFAULT_LOOK_BACK_PERIOD,
            14.0,
        ),
        (
            "an account-default preference",
            Some(json!({"resourceType": "RdsDBInstance", "lookBackPeriod": "DAYS_32"})),
            "DAYS_32",
            32.0,
        ),
        (
            "an account-scoped preference beats the account default",
            Some(json!({
                "resourceType": "RdsDBInstance",
                "scope": {"name": "AccountId", "value": own_account()},
                "lookBackPeriod": "DAYS_93",
            })),
            "DAYS_93",
            93.0,
        ),
    ];

    for (case, preference, wanted_preference, wanted_days) in rows {
        if let Some(preference) = preference {
            let (status, body) =
                call(&router, "PutRecommendationPreferences", preference.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: writing {preference} expected 200, got {status}: {body}"
            );
        }
        let reported = reported_for(&router, case, &database).await;
        assert_eq!(
            reported["effectiveRecommendationPreferences"]["lookBackPeriod"],
            json!(wanted_preference),
            "case {case:?}: expected lookBackPeriod {wanted_preference}, got {reported}"
        );
        assert_eq!(
            reported["lookbackPeriodInDays"],
            json!(wanted_days),
            "case {case:?}: expected {wanted_days} days, got {reported}"
        );
    }
}

/// `recommendationPreferences` "Describes the recommendation preferences to return in the
/// response", so the CPU vendor architecture a request asks for is the one its answer reports
/// as effective — and a request asking for none reports none.
#[tokio::test]
async fn the_requested_cpu_vendor_architecture_is_reported_as_effective() {
    let (router, _engine, corpus) = emulator_with_corpus().await;
    let database = corpus[0].arn_in(REGION);

    // (case, the recommendationPreferences requested, the member expected) — the values are
    // the model's own `CpuVendorArchitecture` enum.
    let rows: Vec<(&str, Value, Value)> = vec![
        ("none requested", json!({}), Value::Null),
        (
            "the current architecture",
            json!({"recommendationPreferences": {"cpuVendorArchitectures": ["CURRENT"]}}),
            json!(["CURRENT"]),
        ),
        (
            "both architectures",
            json!({"recommendationPreferences": {
                "cpuVendorArchitectures": ["AWS_ARM64", "CURRENT"],
            }}),
            json!(["AWS_ARM64", "CURRENT"]),
        ),
        (
            "an empty list asks for none",
            json!({"recommendationPreferences": {"cpuVendorArchitectures": []}}),
            Value::Null,
        ),
    ];

    for (case, mut request, wanted) in rows {
        request["resourceArns"] = json!([database.clone()]);
        let body = get_rds(&router, case, request).await;
        let reported = &body["rdsDBRecommendations"][0];
        assert_eq!(
            reported["effectiveRecommendationPreferences"]["cpuVendorArchitectures"], wanted,
            "case {case:?}: expected {wanted}, got {reported}"
        );
    }
}

/// Paging: each page carries at most `maxResults` recommendations, the token resumes exactly
/// where the page stopped, and walking every page reports the selection once and completely.
#[tokio::test]
async fn the_page_walks_the_selection_once() {
    let (router, _engine, corpus) = emulator_with_corpus().await;
    let all = get_rds(&router, "the whole selection", json!({})).await;
    let everything = reported_arns(&all);
    assert_eq!(
        everything.len(),
        corpus.len() + 1,
        "the recorded corpus plus the seeded database, got {all}"
    );

    for page_size in [1_usize, 2, 3, everything.len(), everything.len() + 1] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let body = get_rds(&router, &case, request).await;
            let page = reported_arns(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size}, got {body}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= everything.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, everything,
            "case {case:?}: the walk must report the selection once, in order"
        );
        let unique: BTreeSet<&String> = walked.iter().collect();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case {case:?}: no recommendation may be reported twice"
        );
    }
}

/// A `nextToken` is good only for the selection it was issued for: resuming another one — or
/// a string this service never issued — is refused rather than answered with a page of a
/// different result set.
#[tokio::test]
async fn a_token_from_another_selection_is_refused() {
    let (router, _engine, _corpus) = emulator_with_corpus().await;
    let first = get_rds(&router, "the first page", json!({"maxResults": 1})).await;
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a token to continue with, got {first}"))
        .to_string();

    // (case, the request the token is replayed in)
    let rows: Vec<(&str, Value)> = vec![
        (
            "a different resource selection",
            json!({
                "maxResults": 1,
                "resourceArns": [db_arn(own_account(), "orders-primary")],
                "nextToken": token.clone(),
            }),
        ),
        (
            "a different filter selection",
            json!({
                "maxResults": 1,
                "filters": [{"name": "InstanceFinding", "values": ["Optimized"]}],
                "nextToken": token.clone(),
            }),
        ),
        (
            "a different recommendationPreferences selection",
            json!({
                "maxResults": 1,
                "recommendationPreferences": {"cpuVendorArchitectures": ["CURRENT"]},
                "nextToken": token.clone(),
            }),
        ),
        (
            "a token this service never issued",
            json!({"maxResults": 1, "nextToken": "not-a-token"}),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = call(&router, "GetRDSDatabaseRecommendations", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {request} must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
    }

    // The token still resumes the walk it was issued for.
    let second = get_rds(
        &router,
        "the token's own selection",
        json!({"maxResults": 1, "nextToken": token}),
    )
    .await;
    assert_eq!(
        reported_arns(&second).len(),
        1,
        "the second page of the walk the token belongs to, got {second}"
    );
}

/// A request outside the vocabulary or the bounds the model declares is refused, with the
/// parameter named — never answered with a page that silently matched nothing.
#[tokio::test]
async fn a_request_outside_the_model_is_refused() {
    let (router, _engine, _corpus) = emulator_with_corpus().await;

    // (case, request, the error code expected)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "two account ids: only one is allowed per request",
            json!({"accountIds": [own_account(), MEMBER_ACCOUNT]}),
            "InvalidParameterValueException",
        ),
        (
            "an account id that is not 12 digits",
            json!({"accountIds": ["1234"]}),
            "InvalidParameterValueException",
        ),
        (
            "a resource that is not an ARN",
            json!({"resourceArns": ["orders-writer"]}),
            "InvalidParameterValueException",
        ),
        (
            "a filter with no name",
            json!({"filters": [{"values": ["Optimized"]}]}),
            "InvalidParameterValueException",
        ),
        (
            "a storage finding supplied to the instance-finding filter",
            json!({"filters": [{"name": "InstanceFinding", "values": ["NotOptimized"]}]}),
            "InvalidParameterValueException",
        ),
        (
            "a storage reason code supplied to the instance reason-code filter",
            json!({"filters": [
                {"name": "InstanceFindingReasonCode", "values": ["EBSVolumeIOPSOverprovisioned"]},
            ]}),
            "InvalidParameterValueException",
        ),
        (
            "a value the Idle filter does not declare",
            json!({"filters": [{"name": "Idle", "values": ["Maybe"]}]}),
            "InvalidParameterValueException",
        ),
    ];

    for (case, request, wanted) in rows {
        let (status, body) = call(&router, "GetRDSDatabaseRecommendations", request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {request} must be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            wanted,
            "case {case:?}: expected {wanted}, got {body}"
        );
    }

    // `maxResults` is modelled `0..1000`: one past the ceiling is refused at the request
    // boundary, and the ceiling itself is accepted.
    let (status, body) = call(
        &router,
        "GetRDSDatabaseRecommendations",
        json!({"maxResults": MAX_PAGE_SIZE + 1}),
    )
    .await;
    assert_eq!(
        status, 400,
        "maxResults={}: outside the modelled range 0..{MAX_PAGE_SIZE}, got {status}: {body}",
        MAX_PAGE_SIZE + 1
    );
    let body = get_rds(
        &router,
        "maxResults at the modelled ceiling",
        json!({"maxResults": MAX_PAGE_SIZE}),
    )
    .await;
    assert!(
        body["nextToken"].is_null(),
        "the whole selection fits in one page of {MAX_PAGE_SIZE}, got {body}"
    );
}

/// A requested resource this service holds no database recommendation for is ABSENT from the
/// answer, whatever kind of resource its ARN names — a well-formed ARN is never refused, and
/// never reported as a per-resource failure either.
#[tokio::test]
async fn a_requested_resource_with_no_database_recommendation_is_absent() {
    let (router, _engine, corpus) = emulator_with_corpus().await;
    let stored = corpus[0].arn_in(REGION);

    // (case, the requested ARN)
    let rows: Vec<(&str, String)> = vec![
        (
            "a DB instance nothing analysed",
            db_arn(own_account(), "no-such-database"),
        ),
        (
            "a DB cluster nothing analysed",
            cluster_arn(own_account(), "no-such-cluster"),
        ),
        (
            "an analysed resource that is not a database",
            instance_arn("i-0123456789abcdef0"),
        ),
        (
            "a resource of another service entirely",
            arn("dynamodb", own_account(), "table/orders"),
        ),
    ];

    for (case, requested) in rows {
        let body = get_rds(
            &router,
            case,
            json!({"resourceArns": [requested.clone(), stored.clone()]}),
        )
        .await;
        assert_eq!(
            reported_arns(&body),
            vec![stored.clone()],
            "case {case:?}: only the analysed database is reported, got {body}"
        );
        assert_eq!(
            body["errors"],
            json!([]),
            "case {case:?}: {requested} has no recommendation yet, which is no error of the \
             request, got {body}"
        );
    }
}

/// Compute Optimizer is a REGIONAL service: the endpoint a request addresses is the region
/// whose resources it asks about, so every ARN the answer names carries that region — and a
/// resource named by the ARN of another region is a resource this endpoint holds no
/// recommendation for.
#[tokio::test]
async fn the_reported_arn_names_the_region_the_endpoint_serves() {
    let (router, engine) = emulator_with_engine();
    opt_in(&router).await;
    record(&engine, &corpus());
    // The catalog is read only once the analysis the enrollment started is due.
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;

    // The regions these rows are read at: the harness default, another commercial region and
    // one in another continent.
    for region in [REGION, "us-west-2", "eu-central-1"] {
        let (status, body) =
            call_in_region(&router, region, "GetRDSDatabaseRecommendations", json!({})).await;
        assert_eq!(
            status, 200,
            "region {region:?}: expected 200, got {status}: {body}"
        );
        let reported = reported_arns(&body);
        assert_eq!(
            reported.len(),
            corpus().len(),
            "region {region:?}: every analysed database is this region's, got {body}"
        );
        for arn in &reported {
            let prefix = format!("arn:aws:rds:{region}:");
            assert!(
                arn.starts_with(&prefix),
                "region {region:?}: expected an ARN starting {prefix:?}, got {arn:?}"
            );
        }

        // Selecting by the ARN this endpoint reports finds the database; the same resource
        // named in another region is not one of this endpoint's.
        let (status, selected) = call_in_region(
            &router,
            region,
            "GetRDSDatabaseRecommendations",
            json!({"resourceArns": [reported[0].clone()]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region:?}: expected 200, got {status}: {selected}"
        );
        assert_eq!(
            reported_arns(&selected),
            vec![reported[0].clone()],
            "region {region:?}: expected the named database, got {selected}"
        );
        let elsewhere = arn("rds", own_account(), "db:orders-primary")
            .replace(&format!(":{REGION}:"), ":ap-south-1:");
        let (status, body) = call_in_region(
            &router,
            region,
            "GetRDSDatabaseRecommendations",
            json!({"resourceArns": [elsewhere.clone()]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region:?}: a well-formed ARN is never refused, got {status}: {body}"
        );
        assert_eq!(
            reported_arns(&body),
            Vec::<String>::new(),
            "region {region:?}: a database of another region has no recommendation here, \
             got {body}"
        );
        assert_eq!(
            body["errors"],
            json!([]),
            "region {region:?}: an ARN with no recommendation here is no error of the \
             request, got {body}"
        );
    }
}

/// The service analyses an account's resources only once it has opted in, and reports that
/// analysis only once the analysis its enrollment started is due: an account that never
/// opted in — or that opted back out — reads no recommendations at all, and one that has
/// just opted in reads no analysis yet.
#[tokio::test]
async fn the_read_waits_for_the_enrollment_and_the_analysis() {
    let (router, engine) = emulator_with_engine();
    record(&engine, &corpus());

    for case in ["before opting in", "after opting back out"] {
        let (status, body) = call(&router, "GetRDSDatabaseRecommendations", json!({})).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected 403, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "OptInRequiredException",
            "case {case:?}: expected OptInRequiredException, got {body}"
        );

        if case == "before opting in" {
            opt_in(&router).await;
            let fresh = get_rds(&router, "just opted in", json!({})).await;
            assert_eq!(
                reported_arns(&fresh),
                Vec::<String>::new(),
                "just opted in: the analysis is not due yet, so nothing is analysed: {fresh}"
            );

            tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
            let body = get_rds(&router, "the analysis is due", json!({})).await;
            assert!(
                !reported_arns(&body).is_empty(),
                "an opted-in account whose analysis is due reads its analysed databases, \
                 got {body}"
            );
            let (status, body) = call(
                &router,
                "UpdateEnrollmentStatus",
                json!({"status": "Inactive"}),
            )
            .await;
            assert_eq!(status, 200, "opting out: expected 200, got {status}: {body}");
        }
    }
}
