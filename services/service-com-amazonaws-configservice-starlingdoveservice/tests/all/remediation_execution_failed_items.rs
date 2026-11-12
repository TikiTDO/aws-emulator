// SPDX-License-Identifier: CC0-1.0

//! `StartRemediationExecution`: the per-resource outcome an accepted request reports.
//!
//! The rule pinned here: on a request this operation ACCEPTS, the member that reports
//! which of the submitted items could not be started is always present in the response
//! — an EMPTY list when every one of them started — so a caller reconciling what it
//! submitted against what failed never has to tell an absent member from an empty one.
//! The explanatory message, by contrast, is only there when there is a failure to
//! explain.
//!
//! The table spans what the rule generalises over: how many resource keys one request
//! carries (one, a few, and the published maximum of `ResourceKeys`), which resource
//! types they name, and how many of them fail — none, one, the middle one of several,
//! and all of them.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    EvaluationResultEntry, EVALUATION_MODE_DETECTIVE,
};
use service_com_amazonaws_configservice_starlingdoveservice::ComAmazonawsConfigserviceStarlingdoveserviceState as ConfigState;

use crate::config_setup_and_rule_loop::{
    managed_rule, with_recorder_and_channel, with_verdicts, Emulator,
};

/// The verdict a rule has to hold about a resource before there is anything to
/// remediate, from `smithy_query(query="shape", shape="ComplianceType")`.
const NON_COMPLIANT: &str = "NON_COMPLIANT";

/// The remediation target a configuration names: an SSM automation document, the only
/// published `RemediationTargetType`
/// (`smithy_query(query="shape", shape="RemediationTargetType")`).
const TARGET_TYPE: &str = "SSM_DOCUMENT";
/// See [`TARGET_TYPE`].
const TARGET_ID: &str = "AWS-EnableS3BucketVersioning";

/// The `Parameters` key carrying the role a remediation runs as, which
/// `PutRemediationConfigurations` requires.
const AUTOMATION_ASSUME_ROLE: &str = "AutomationAssumeRole";
/// A role ARN a remediation hands to SSM; its value is only ever read back.
const AUTOMATION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/config-remediation";

/// Resource types a key names, from `smithy_query(query="shape", shape="ResourceType")`;
/// `AWS::::Account` is the account-level type Config uses for the account itself
/// (`ExternalEvaluation$ComplianceResourceType`), which that value set does not list.
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`S3_BUCKET`].
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`S3_BUCKET`].
const ACCOUNT: &str = "AWS::::Account";

/// The published maximum of `ResourceKeys`
/// (`smithy_query(query="shape", shape="ResourceKeys")`: `@length` min 1, max 100), and
/// what the operation documents ("You can specify up to 100 resource keys per request").
const MAX_RESOURCE_KEYS: usize = 100;

/// An emulator holding the rule `name` with a remediation configuration attached, which
/// is what makes a `StartRemediationExecution` for it acceptable at all.
async fn emulator_with_remediated_rule(name: &str) -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator.ok("PutConfigRule", &managed_rule(name)).await;
    emulator
        .ok(
            "PutRemediationConfigurations",
            &json!({
                "RemediationConfigurations": [{
                    "ConfigRuleName": name,
                    "TargetType": TARGET_TYPE,
                    "TargetId": TARGET_ID,
                    "Parameters": {
                        AUTOMATION_ASSUME_ROLE: {
                            "StaticValue": { "Values": [AUTOMATION_ROLE_ARN] },
                        },
                    },
                }],
            }),
        )
        .await;
    emulator
}

/// The `StartRemediationExecution` body for `name` over `keys`.
fn start(name: &str, keys: &[(String, String)]) -> Value {
    json!({
        "ConfigRuleName": name,
        "ResourceKeys": keys
            .iter()
            .map(|(resource_type, resource_id)| json!({
                "resourceType": resource_type,
                "resourceId": resource_id,
            }))
            .collect::<Vec<Value>>(),
    })
}

/// Deliver the rule's own COMPLIANT verdict for `keys`, the way a rule's evaluation
/// delivers one: a resource the rule already places as compliant has nothing to
/// remediate, so its key cannot start.
fn already_compliant(emulator: &Emulator, rule: &str, keys: &[(String, String)]) {
    let state = emulator.engine.state::<ConfigState>();
    let mut store = state.lock();
    let entry = store
        .config_rules
        .get_mut(rule)
        .unwrap_or_else(|| panic!("the rule {rule:?} was created"));
    for (resource_type, resource_id) in keys {
        entry.record_evaluation_result(EvaluationResultEntry::new(
            resource_type,
            resource_id,
            EVALUATION_MODE_DETECTIVE,
            "COMPLIANT",
            std::time::SystemTime::UNIX_EPOCH,
        ));
    }
}

/// The `(resourceType, resourceId)` pairs a response reports as failed, sorted.
fn failed_pairs(case: &str, answered: &Value) -> Vec<(String, String)> {
    let reported = answered
        .get("FailedItems")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected FailedItems to be reported, got {answered}")
        });
    let mut pairs: Vec<(String, String)> = reported
        .iter()
        .map(|key| {
            (
                key["resourceType"].as_str().unwrap_or_default().to_string(),
                key["resourceId"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    pairs.sort();
    pairs
}

/// One row: the keys the request carries, and which of them the rule already places as
/// compliant (so those, and only those, are the ones reported back).
struct FailedItemsCase {
    case: &'static str,
    keys: Vec<(String, String)>,
    compliant: Vec<(String, String)>,
}

/// A key, spelled the way the table wants it.
fn key(resource_type: &str, resource_id: &str) -> (String, String) {
    (resource_type.to_string(), resource_id.to_string())
}

#[tokio::test]
async fn an_accepted_start_always_reports_the_keys_that_failed() {
    let hundred: Vec<(String, String)> = (0..MAX_RESOURCE_KEYS)
        .map(|index| key(S3_BUCKET, &format!("config-bucket-{index:03}")))
        .collect();
    // The maximum-sized request with its last key already compliant: the bound carries
    // a failure too, not only an empty report.
    let mut hundred_with_one_compliant = hundred.clone();
    hundred_with_one_compliant.reverse();
    let last_of_the_hundred = hundred
        .last()
        .cloned()
        .expect("the maximum is at least one key");

    let cases: Vec<FailedItemsCase> = vec![
        FailedItemsCase {
            case: "one key, nothing in the way of it",
            keys: vec![key(S3_BUCKET, "config-bucket")],
            compliant: Vec::new(),
        },
        FailedItemsCase {
            case: "the account-level key, nothing in the way of it",
            keys: vec![key(ACCOUNT, "000000000000")],
            compliant: Vec::new(),
        },
        FailedItemsCase {
            case: "three keys of three resource types, all startable",
            keys: vec![
                key(S3_BUCKET, "config-bucket"),
                key(EC2_VOLUME, "vol-0001"),
                key(ACCOUNT, "000000000000"),
            ],
            compliant: Vec::new(),
        },
        FailedItemsCase {
            case: "the published maximum of resource keys, all startable",
            keys: hundred.clone(),
            compliant: Vec::new(),
        },
        FailedItemsCase {
            case: "two keys, the first already compliant",
            keys: vec![key(S3_BUCKET, "config-bucket"), key(EC2_VOLUME, "vol-0001")],
            compliant: vec![key(S3_BUCKET, "config-bucket")],
        },
        FailedItemsCase {
            case: "three keys, the middle one already compliant",
            keys: vec![
                key(S3_BUCKET, "config-bucket"),
                key(EC2_VOLUME, "vol-0001"),
                key(ACCOUNT, "000000000000"),
            ],
            compliant: vec![key(EC2_VOLUME, "vol-0001")],
        },
        FailedItemsCase {
            case: "every key already compliant",
            keys: vec![key(S3_BUCKET, "config-bucket"), key(EC2_VOLUME, "vol-0001")],
            compliant: vec![key(S3_BUCKET, "config-bucket"), key(EC2_VOLUME, "vol-0001")],
        },
        FailedItemsCase {
            case: "the maximum-sized request with one key already compliant",
            keys: hundred_with_one_compliant,
            compliant: vec![last_of_the_hundred],
        },
    ];

    for FailedItemsCase {
        case,
        keys,
        compliant,
    } in cases
    {
        let name = "failed-items-rule";
        let emulator = emulator_with_remediated_rule(name).await;
        // Every key the request names is one the rule FOUND non-compliant, which is what
        // makes it startable at all; the ones this row calls compliant are then placed as
        // compliant instead, so they are the only keys with nothing to remediate.
        with_verdicts(&emulator, name, NON_COMPLIANT, &keys);
        already_compliant(&emulator, name, &compliant);

        let answered = emulator
            .ok("StartRemediationExecution", &start(name, &keys))
            .await;

        let mut expected = compliant.clone();
        expected.sort();
        let reported = failed_pairs(case, &answered);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected FailedItems to report exactly {expected:?}, got {reported:?} \
             in {answered}"
        );

        // The message explains a failure, so it is there exactly when there is one.
        assert_eq!(
            answered.get("FailureMessage").is_some(),
            !expected.is_empty(),
            "case {case:?}: expected FailureMessage present={}, got {answered}",
            !expected.is_empty(),
        );

        // What the report says is what the service did: one execution for every key that
        // was not reported back.
        let statuses = emulator
            .ok(
                "DescribeRemediationExecutionStatus",
                &json!({ "ConfigRuleName": name }),
            )
            .await;
        let started = statuses["RemediationExecutionStatuses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected statuses, got {statuses}"))
            .len();
        assert_eq!(
            started,
            keys.len() - expected.len(),
            "case {case:?}: expected {} execution(s) invoked, got {started}",
            keys.len() - expected.len(),
        );
    }
}

#[tokio::test]
async fn the_empty_report_is_read_back_on_every_accepted_call() {
    // A caller that submits, waits for the execution to finish and submits again reads
    // the same empty report both times: the member is a property of an accepted
    // response, not of the first one.
    let name = "repeat-start-rule";
    let emulator = emulator_with_remediated_rule(name).await;
    let keys = vec![key(S3_BUCKET, "config-bucket")];
    with_verdicts(&emulator, name, NON_COMPLIANT, &keys);

    for case in ["the first accepted call", "the call after it finished"] {
        let answered = emulator
            .ok("StartRemediationExecution", &start(name, &keys))
            .await;
        assert_eq!(
            answered.get("FailedItems").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {case:?}: expected FailedItems reported empty, got {answered}"
        );
        tokio::time::sleep(
            service_com_amazonaws_configservice_starlingdoveservice::state::REMEDIATION_EXECUTION_COMPLETES_AFTER
                + std::time::Duration::from_millis(50),
        )
        .await;
    }
}
