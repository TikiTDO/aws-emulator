// SPDX-License-Identifier: CC0-1.0

//! What a DEPLOYED conformance pack's rules report when the rule it deployed is one placed
//! from a RECORDED resource.
//!
//! The rule under test, the other half of `conformance_pack_rule_evaluation`'s: **a Config
//! Managed rule a conformance pack's deployment created is evaluated by the same pass that
//! evaluates the account's own rules — so once the configuration recorder has recorded a
//! resource the rule's scope covers, the pack's rule-level compliance report
//! (`DescribeConformancePackCompliance`) and its pack-level summary
//! (`GetConformancePackComplianceSummary`) both report the verdict that rule placed that
//! resource at, and nothing else.**
//!
//! Config and S3 are mounted on one engine, reusing the harness
//! `config_recorded_resource_compliance` builds: the recorder reads the buckets it records
//! from S3's own API, and the rule's verdict is computed from the configuration item it
//! recorded for one.
//!
//! The rows differ along what the rule generalises over: the recorded state the verdict is
//! computed from (versioning never set, suspended, enabled) and therefore the verdict the
//! pack rolls up.

use std::time::{Duration, Instant};

use serde_json::json;
use service_com_amazonaws_configservice_starlingdoveservice::state::conformance_pack_rule_name;

use crate::config_recorded_resource_compliance::Emulator;

/// The managed rule the pack deploys, a `Source$SourceIdentifier` value from the Config
/// managed-rule list: "checks if versioning is enabled for your S3 buckets".
const MANAGED_RULE: &str = "S3_BUCKET_VERSIONING_ENABLED";

/// The resource type a conformance pack template declares a Config rule as.
const CONFIG_RULE_TYPE: &str = "AWS::Config::ConfigRule";

/// The resource type Config records an S3 bucket under.
const BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// `ComplianceType` values this suite reads verdicts as.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";

/// The bucket versioning statuses `PutBucketVersioning` takes
/// (`com.amazonaws.s3#BucketVersioningStatus`).
const ENABLED: &str = "Enabled";
/// See [`ENABLED`].
const SUSPENDED: &str = "Suspended";

/// How long a poll waits for the deployment, the recording pass and the evaluation the
/// clock owes.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(20);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A template declaring one Config rule of [`MANAGED_RULE`], scoped to the resource type
/// it evaluates.
fn template(rule_name: &str) -> String {
    format!(
        "Resources:\n  Rule0:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
         ConfigRuleName: {rule_name}\n      Scope:\n        ComplianceResourceTypes:\n        \
         - {BUCKET_RESOURCE_TYPE}\n      Source:\n        Owner: AWS\n        \
         SourceIdentifier: {MANAGED_RULE}\n"
    )
}

/// Deploy `pack` and answer the name Config gave the rule it declared, once the pack
/// reports `CREATE_COMPLETE`.
async fn deploy(case: &str, emulator: &Emulator, pack: &str, rule_name: &str) -> String {
    emulator
        .config(
            "PutConformancePack",
            &json!({ "ConformancePackName": pack, "TemplateBody": template(rule_name) }),
        )
        .await;
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let status = emulator
            .config(
                "DescribeConformancePackStatus",
                &json!({ "ConformancePackNames": [pack] }),
            )
            .await;
        let detail = &status["ConformancePackStatusDetails"][0];
        if detail["ConformancePackState"] == json!("CREATE_COMPLETE") {
            let id = detail["ConformancePackId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected the pack id, got {status}"));
            return conformance_pack_rule_name(rule_name, id);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the pack's deployment to settle, got {status}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `(ConfigRuleName, ComplianceType)` rows the pack's rule-level compliance reports.
async fn rule_rows(emulator: &Emulator, pack: &str) -> Vec<(String, String)> {
    let body = emulator
        .config(
            "DescribeConformancePackCompliance",
            &json!({ "ConformancePackName": pack }),
        )
        .await;
    body["ConformancePackRuleComplianceList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            (
                entry["ConfigRuleName"].as_str().unwrap_or_default().to_string(),
                entry["ComplianceType"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Poll the pack's rule-level compliance until a row reports a verdict either way, and
/// answer the rows.
async fn await_rule_rows(case: &str, emulator: &Emulator, pack: &str) -> Vec<(String, String)> {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let rows = rule_rows(emulator, pack).await;
        if rows
            .iter()
            .any(|(_, compliance)| compliance == COMPLIANT || compliance == NON_COMPLIANT)
        {
            return rows;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the pack's rule to be evaluated, got {rows:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `ConformancePackComplianceStatus` the pack's own summary reports.
async fn pack_status(case: &str, emulator: &Emulator, pack: &str) -> String {
    let body = emulator
        .config(
            "GetConformancePackComplianceSummary",
            &json!({ "ConformancePackNames": [pack] }),
        )
        .await;
    body["ConformancePackComplianceSummaryList"][0]["ConformancePackComplianceStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the pack's compliance, got {body}"))
        .to_string()
}

#[tokio::test]
async fn a_packs_rule_reports_the_verdict_it_placed_a_recorded_resource_at() {
    // The rows differ along the recorded state the verdict is computed from.
    let cases: [(&str, Option<&str>, &str); 3] = [
        ("versioning never set", None, NON_COMPLIANT),
        ("versioning suspended", Some(SUSPENDED), NON_COMPLIANT),
        ("versioning enabled", Some(ENABLED), COMPLIANT),
    ];

    for (case, versioning, expected) in cases {
        let emulator = Emulator::new();
        emulator.recording().await;
        let bucket = "pack-rule-bucket";
        emulator.create_bucket(bucket, &[]).await;
        if let Some(status) = versioning {
            emulator.set_versioning(bucket, status).await;
        }

        let pack = "recorded-resource-pack";
        let rule = deploy(case, &emulator, pack, "bucket-versioning").await;
        let rows = await_rule_rows(case, &emulator, pack).await;
        assert_eq!(
            rows,
            vec![(rule.clone(), expected.to_string())],
            "case {case:?}: expected the pack's rule to report {expected}, got {rows:?}"
        );

        // The pack's own summary is the roll-up of exactly those rows.
        let status = pack_status(case, &emulator, pack).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected the pack's compliance to be {expected}, got {status}"
        );

        // And the result behind the row names the recorded resource it was placed on.
        let details = emulator
            .config(
                "GetConformancePackComplianceDetails",
                &json!({ "ConformancePackName": pack }),
            )
            .await;
        let identifier = &details["ConformancePackRuleEvaluationResults"][0]
            ["EvaluationResultIdentifier"]["EvaluationResultQualifier"];
        assert_eq!(
            (
                identifier["ResourceType"].as_str(),
                identifier["ResourceId"].as_str()
            ),
            (Some(BUCKET_RESOURCE_TYPE), Some(bucket)),
            "case {case:?}: expected the result to name the recorded bucket, got {details}"
        );
    }
}
