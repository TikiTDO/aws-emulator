// SPDX-License-Identifier: CC0-1.0

//! `StartRemediationExecution` and `DescribeRemediationExecutionStatus`: running a Config
//! rule's remediation action, and reporting what it is doing.
//!
//! The rule pinned here: a request to remediate a rule that has NO remediation
//! configuration attached is refused with `NoSuchRemediationConfigurationException` —
//! the one error the operation models for that precondition ("You specified an Config
//! rule without a remediation configuration") — whatever the resource keys name, and a
//! rule that HAS one invokes an execution per resource key, which
//! `DescribeRemediationExecutionStatus` then reports moving from `QUEUED` through
//! `IN_PROGRESS` to `SUCCEEDED` as the clock makes each move due.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines — over a TABLE of inputs
//! rather than one recorded call: the rows differ along which rule the request names
//! (one with a remediation configuration, one without, one that does not exist at all),
//! along the resource type and id each key carries (including the account-level type,
//! which the published `ResourceType` value set does not list, and a key that names no
//! resource at all), and along how many keys one request carries — both published bounds
//! of `ResourceKeys` and one past each of them.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    REMEDIATION_EXECUTION_COMPLETES_AFTER, REMEDIATION_EXECUTION_STARTS_AFTER,
    REMEDIATION_EXECUTION_STATE_IN_PROGRESS, REMEDIATION_EXECUTION_STATE_QUEUED,
    REMEDIATION_EXECUTION_STATE_SUCCEEDED,
};

use crate::config_setup_and_rule_loop::{
    managed_rule, with_recorder_and_channel, with_verdicts, Emulator,
};

/// The verdict a rule has to hold about a resource before there is anything to
/// remediate, from `smithy_query(query="shape", shape="ComplianceType")`.
const NON_COMPLIANT: &str = "NON_COMPLIANT";

/// The remediation target a configuration names: an SSM automation document, the only
/// published `RemediationTargetType`.
const TARGET_TYPE: &str = "SSM_DOCUMENT";
/// See [`TARGET_TYPE`].
const TARGET_ID: &str = "AWS-EnableS3BucketVersioning";

/// The `Parameters` key carrying the role a remediation runs as, which
/// `PutRemediationConfigurations` requires ("you need to provide a value for
/// automationAssumeRole or use a value in the assumeRole field").
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

/// An emulator holding the rule `name`, with a remediation configuration attached when
/// `remediated`.
async fn emulator_with_rule(name: &str, remediated: bool) -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator.ok("PutConfigRule", &managed_rule(name)).await;
    if remediated {
        let answered = emulator
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
        assert_eq!(
            answered["FailedBatches"].as_array().map(Vec::len),
            Some(0),
            "the remediation configuration of {name:?} is stored, got {answered}"
        );
    }
    emulator
}

/// Record the rule's own NON_COMPLIANT finding for every key of `keys` that names a
/// resource: a remediation acts on a FINDING, so a resource the rule holds no
/// non-compliant evaluation result for starts nothing.
fn found_non_compliant(emulator: &Emulator, rule: &str, keys: &[(&str, &str)]) {
    let resources: Vec<(String, String)> = keys
        .iter()
        .filter(|(resource_type, resource_id)| {
            !resource_type.trim().is_empty() && !resource_id.trim().is_empty()
        })
        .map(|(resource_type, resource_id)| {
            ((*resource_type).to_string(), (*resource_id).to_string())
        })
        .collect();
    with_verdicts(emulator, rule, NON_COMPLIANT, &resources);
}

/// The `StartRemediationExecution` body for `name` over `keys`.
fn start(name: &str, keys: &[(&str, &str)]) -> Value {
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

/// The `(resourceType, resourceId, State)` of every execution reported for `name`.
async fn reported(case: &str, emulator: &Emulator, name: &str) -> Vec<(String, String, String)> {
    let body = emulator
        .ok(
            "DescribeRemediationExecutionStatus",
            &json!({ "ConfigRuleName": name }),
        )
        .await;
    let mut reported: Vec<(String, String, String)> = body["RemediationExecutionStatuses"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected RemediationExecutionStatuses, got {body}")
        })
        .iter()
        .map(|status| {
            (
                status["ResourceKey"]["resourceType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                status["ResourceKey"]["resourceId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                status["State"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    reported.sort();
    reported
}

/// One row of the refusal table: which rule the request names, whether that rule
/// exists and whether it has a remediation configuration, the resource keys it carries,
/// and the error code expected (`None` where the request is served).
struct StartCase {
    case: &'static str,
    exists: bool,
    remediated: bool,
    keys: &'static [(&'static str, &'static str)],
    expected: Option<&'static str>,
}

#[tokio::test]
async fn a_rule_with_no_remediation_configuration_is_refused_as_that() {
    let cases: [StartCase; 6] = [
        StartCase {
            case: "the account-level key the recording uses, on a rule with no remediation",
            exists: true,
            remediated: false,
            keys: &[(ACCOUNT, "000000000000")],
            expected: Some("NoSuchRemediationConfigurationException"),
        },
        StartCase {
            case: "a native resource key, on a rule with no remediation",
            exists: true,
            remediated: false,
            keys: &[(S3_BUCKET, "config-bucket")],
            expected: Some("NoSuchRemediationConfigurationException"),
        },
        StartCase {
            case: "several keys, on a rule with no remediation",
            exists: true,
            remediated: false,
            keys: &[(S3_BUCKET, "config-bucket"), (EC2_VOLUME, "vol-0001")],
            expected: Some("NoSuchRemediationConfigurationException"),
        },
        StartCase {
            case: "a rule this account does not hold has no remediation either",
            exists: false,
            remediated: false,
            keys: &[(ACCOUNT, "000000000000")],
            expected: Some("NoSuchRemediationConfigurationException"),
        },
        StartCase {
            case: "the sibling rule that does have a remediation configuration",
            exists: true,
            remediated: true,
            keys: &[(ACCOUNT, "000000000000")],
            expected: None,
        },
        StartCase {
            case: "a native resource key, on the remediated rule",
            exists: true,
            remediated: true,
            keys: &[(S3_BUCKET, "config-bucket")],
            expected: None,
        },
    ];

    for StartCase {
        case,
        exists,
        remediated,
        keys,
        expected,
    } in cases
    {
        let name = "remediated-rule";
        let emulator = if exists {
            let emulator = emulator_with_rule(name, remediated).await;
            found_non_compliant(&emulator, name, keys);
            emulator
        } else {
            let emulator = Emulator::new();
            with_recorder_and_channel(&emulator).await;
            emulator
        };

        match expected {
            Some(expected) => {
                let (status, code) = emulator
                    .refused("StartRemediationExecution", &start(name, keys))
                    .await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {case:?}: expected HTTP 400 {expected}, got {status} {code}"
                );
            }
            None => {
                let answered = emulator
                    .ok("StartRemediationExecution", &start(name, keys))
                    .await;
                assert_eq!(
                    answered.get("FailedItems").and_then(Value::as_array),
                    Some(&Vec::new()),
                    "case {case:?}: every key started, so FailedItems is reported empty, got \
                     {answered}"
                );
                let states = reported(case, &emulator, name).await;
                assert_eq!(
                    states.len(),
                    keys.len(),
                    "case {case:?}: expected one execution per key, got {states:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_execution_moves_from_queued_through_running_to_succeeded() {
    let name = "moving-rule";
    let emulator = emulator_with_rule(name, true).await;
    let keys: &[(&str, &str)] = &[(S3_BUCKET, "config-bucket"), (EC2_VOLUME, "vol-0001")];
    found_non_compliant(&emulator, name, keys);

    // Nothing has run yet, so the rule reports no execution at all.
    assert!(
        reported("before the start", &emulator, name)
            .await
            .is_empty(),
        "a rule nothing has remediated reports no execution"
    );

    emulator
        .ok("StartRemediationExecution", &start(name, keys))
        .await;

    // (case, how long to wait before reading, the state every execution is then in).
    // The spans come from the service itself, so the test cannot drift from what it
    // owes.
    let stages: [(&str, Duration, &str); 3] = [
        (
            "straight after the call",
            Duration::ZERO,
            REMEDIATION_EXECUTION_STATE_QUEUED,
        ),
        (
            "once the execution is due to have started",
            REMEDIATION_EXECUTION_STARTS_AFTER + Duration::from_millis(50),
            REMEDIATION_EXECUTION_STATE_IN_PROGRESS,
        ),
        (
            "once it is due to have finished",
            REMEDIATION_EXECUTION_COMPLETES_AFTER + Duration::from_millis(50),
            REMEDIATION_EXECUTION_STATE_SUCCEEDED,
        ),
    ];

    for (case, wait, expected) in stages {
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let states = reported(case, &emulator, name).await;
        let expected_states: Vec<(String, String, String)> = {
            let mut rows: Vec<(String, String, String)> = keys
                .iter()
                .map(|(resource_type, resource_id)| {
                    (
                        (*resource_type).to_string(),
                        (*resource_id).to_string(),
                        expected.to_string(),
                    )
                })
                .collect();
            rows.sort();
            rows
        };
        assert_eq!(
            states, expected_states,
            "case {case:?}: expected {expected_states:?}, got {states:?}"
        );

        // The status read is a REPORT: reading it again must not move anything.
        let again = reported(case, &emulator, name).await;
        assert_eq!(
            again, states,
            "case {case:?}: a second read reports the same executions, got {again:?}"
        );
    }

    // The one step the execution runs is the remediation target the configuration
    // names, and by now it has both started and stopped.
    let body = emulator
        .ok(
            "DescribeRemediationExecutionStatus",
            &json!({ "ConfigRuleName": name }),
        )
        .await;
    let step = &body["RemediationExecutionStatuses"][0]["StepDetails"][0];
    assert_eq!(
        (step["Name"].as_str(), step["State"].as_str()),
        (Some(TARGET_ID), Some("SUCCEEDED")),
        "the step is the remediation target, finished, got {body}"
    );
    assert!(
        !step["StartTime"].is_null() && !step["StopTime"].is_null(),
        "a finished step reports when it ran, got {step}"
    );
}

/// One row of the key-count table: how many resource keys the request carries, and the
/// error code expected (`None` where the request is served).
struct CountCase {
    case: &'static str,
    keys: usize,
    expected: Option<&'static str>,
}

#[tokio::test]
async fn one_execution_is_invoked_per_resource_key_up_to_the_published_maximum() {
    // The counts come from `StartRemediationExecutionRequest$ResourceKeys`
    // (`smithy_query(query="shape", shape="ResourceKeys")`: list of ResourceKey, length
    // 1..100), which the operation's own documentation states as "You can specify up to
    // 100 resource keys per request": both published bounds are exercised, and so is one
    // past each of them.
    let cases: [CountCase; 6] = [
        CountCase {
            case: "no key at all, one below the published minimum",
            keys: 0,
            expected: Some("ValidationException"),
        },
        CountCase {
            case: "the published minimum",
            keys: 1,
            expected: None,
        },
        CountCase {
            case: "a handful",
            keys: 7,
            expected: None,
        },
        CountCase {
            case: "half the published maximum",
            keys: 50,
            expected: None,
        },
        CountCase {
            case: "the published maximum",
            keys: 100,
            expected: None,
        },
        CountCase {
            case: "one key past the published maximum",
            keys: 101,
            expected: Some("ValidationException"),
        },
    ];

    for CountCase {
        case,
        keys,
        expected,
    } in cases
    {
        let name = "counted-rule";
        let emulator = emulator_with_rule(name, true).await;
        // Each key names a DISTINCT resource, so one request carries `keys` of them and
        // the report must hold one execution per key rather than one per request.
        let ids: Vec<String> = (0..keys).map(|index| format!("bucket-{index:04}")).collect();
        let found: Vec<(String, String)> = ids
            .iter()
            .map(|resource_id| (S3_BUCKET.to_string(), resource_id.clone()))
            .collect();
        with_verdicts(&emulator, name, NON_COMPLIANT, &found);
        let request = json!({
            "ConfigRuleName": name,
            "ResourceKeys": ids
                .iter()
                .map(|resource_id| json!({
                    "resourceType": S3_BUCKET,
                    "resourceId": resource_id,
                }))
                .collect::<Vec<Value>>(),
        });

        match expected {
            Some(expected) => {
                let (status, code) = emulator
                    .refused("StartRemediationExecution", &request)
                    .await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {case:?}: expected HTTP 400 {expected} for {keys} key(s), got \
                     {status} {code}"
                );
                let states = reported(case, &emulator, name).await;
                assert!(
                    states.is_empty(),
                    "case {case:?}: a refused request starts nothing, got {states:?}"
                );
            }
            None => {
                emulator.ok("StartRemediationExecution", &request).await;
                let states = reported(case, &emulator, name).await;
                assert_eq!(
                    states.len(),
                    keys,
                    "case {case:?}: expected {keys} execution(s), got {} ({states:?})",
                    states.len()
                );
                let unexpected: Vec<&(String, String, String)> = states
                    .iter()
                    .filter(|(resource_type, resource_id, state)| {
                        resource_type != S3_BUCKET
                            || !ids.contains(resource_id)
                            || state != REMEDIATION_EXECUTION_STATE_QUEUED
                    })
                    .collect();
                assert!(
                    unexpected.is_empty(),
                    "case {case:?}: every execution is queued for a key the request \
                     named, got {unexpected:?}"
                );
            }
        }
    }
}

/// One row of the incomplete-key table: the `(resourceType, resourceId)` a key carries,
/// and the error code expected (`None` where the request is served).
struct KeyCase {
    case: &'static str,
    key: (&'static str, &'static str),
    expected: Option<&'static str>,
}

#[tokio::test]
async fn a_key_that_names_no_resource_is_refused_as_an_invalid_parameter() {
    // `ResourceKey` requires both `resourceType` and `resourceId`
    // (`smithy_query(query="shape", shape="ResourceKey")`: both members required,
    // `resourceId` length 1..768), so a key carrying only whitespace for either names no
    // resource to remediate and the request it belongs to is refused rather than
    // partially started.
    let cases: [KeyCase; 5] = [
        KeyCase {
            case: "an empty resource type",
            key: ("", "config-bucket"),
            expected: Some("InvalidParameterValueException"),
        },
        KeyCase {
            case: "a blank resource type",
            key: ("   ", "config-bucket"),
            expected: Some("InvalidParameterValueException"),
        },
        KeyCase {
            case: "a blank resource id",
            key: (S3_BUCKET, " "),
            expected: Some("InvalidParameterValueException"),
        },
        KeyCase {
            case: "an empty resource id, below its published length minimum",
            key: (S3_BUCKET, ""),
            expected: Some("ValidationException"),
        },
        KeyCase {
            case: "a key naming both, which is served",
            key: (EC2_VOLUME, "vol-0002"),
            expected: None,
        },
    ];

    for KeyCase {
        case,
        key,
        expected,
    } in cases
    {
        let name = "keyed-rule";
        let emulator = emulator_with_rule(name, true).await;
        // The incomplete key travels BESIDE a well-formed one, so what is pinned is the
        // judgement of the request rather than of a single-key request.
        let keys = [(S3_BUCKET, "config-bucket-ok"), key];
        found_non_compliant(&emulator, name, &keys);
        let request = start(name, &keys);

        match expected {
            Some(expected) => {
                let (status, code) = emulator
                    .refused("StartRemediationExecution", &request)
                    .await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, expected),
                    "case {case:?}: expected HTTP 400 {expected}, got {status} {code}"
                );
                let states = reported(case, &emulator, name).await;
                assert!(
                    states.is_empty(),
                    "case {case:?}: a refused request starts nothing, not even for the \
                     well-formed key beside it, got {states:?}"
                );
            }
            None => {
                emulator.ok("StartRemediationExecution", &request).await;
                let states = reported(case, &emulator, name).await;
                assert_eq!(
                    states.len(),
                    2,
                    "case {case:?}: expected one execution per named key, got {states:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn reports_only_the_resource_keys_the_request_names() {
    let name = "selective-rule";
    let emulator = emulator_with_rule(name, true).await;
    let keys: &[(&str, &str)] = &[
        (S3_BUCKET, "config-bucket"),
        (EC2_VOLUME, "vol-0001"),
        (ACCOUNT, "000000000000"),
    ];
    found_non_compliant(&emulator, name, keys);
    emulator
        .ok("StartRemediationExecution", &start(name, keys))
        .await;

    // One row per selection the read asks about: the keys it names and how many
    // executions it then reports.
    struct SelectionCase {
        case: &'static str,
        selected: &'static [(&'static str, &'static str)],
        expected: usize,
    }

    let cases: [SelectionCase; 4] = [
        SelectionCase {
            case: "every key",
            selected: &[],
            expected: 3,
        },
        SelectionCase {
            case: "one key",
            selected: &[(EC2_VOLUME, "vol-0001")],
            expected: 1,
        },
        SelectionCase {
            case: "two of the three",
            selected: &[(EC2_VOLUME, "vol-0001"), (ACCOUNT, "000000000000")],
            expected: 2,
        },
        SelectionCase {
            case: "a resource nothing remediated",
            selected: &[(S3_BUCKET, "other")],
            expected: 0,
        },
    ];

    for SelectionCase {
        case,
        selected,
        expected,
    } in cases
    {
        let mut request = json!({ "ConfigRuleName": name });
        if !selected.is_empty() {
            request["ResourceKeys"] = json!(selected
                .iter()
                .map(|(resource_type, resource_id)| json!({
                    "resourceType": resource_type,
                    "resourceId": resource_id,
                }))
                .collect::<Vec<Value>>());
        }
        let body = emulator
            .ok("DescribeRemediationExecutionStatus", &request)
            .await;
        let statuses = body["RemediationExecutionStatuses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected statuses, got {body}"));
        assert_eq!(
            statuses.len(),
            expected,
            "case {case:?}: expected {expected} execution(s), got {body}"
        );
    }

    // A rule with no remediation configuration has no execution to report either, which
    // is the same refusal the start path makes.
    let (status, code) = emulator
        .refused(
            "DescribeRemediationExecutionStatus",
            &json!({ "ConfigRuleName": "no-such-rule" }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NoSuchRemediationConfigurationException"),
        "expected HTTP 400 NoSuchRemediationConfigurationException, got {status} {code}"
    );
}

#[tokio::test]
async fn a_resource_whose_execution_has_not_finished_is_not_started_again() {
    let name = "in-flight-rule";
    let emulator = emulator_with_rule(name, true).await;
    let keys: &[(&str, &str)] = &[(S3_BUCKET, "config-bucket")];
    found_non_compliant(&emulator, name, keys);
    emulator
        .ok("StartRemediationExecution", &start(name, keys))
        .await;

    // "An existing StartRemediationExecution call for the specified resource keys must
    // complete before you can call the API again", so the second call starts nothing
    // and reports the key back.
    let answered = emulator
        .ok("StartRemediationExecution", &start(name, keys))
        .await;
    let failed = answered["FailedItems"]
        .as_array()
        .unwrap_or_else(|| panic!("expected the key reported back, got {answered}"));
    assert_eq!(
        (
            failed.len(),
            failed[0]["resourceId"].as_str(),
            answered["FailureMessage"].is_string(),
        ),
        (1, Some("config-bucket"), true),
        "the in-flight key is reported with the reason it could not start, got {answered}"
    );

    // Once it has finished, the same resource can be remediated again.
    tokio::time::sleep(REMEDIATION_EXECUTION_COMPLETES_AFTER + Duration::from_millis(50)).await;
    let answered = emulator
        .ok("StartRemediationExecution", &start(name, keys))
        .await;
    assert_eq!(
        answered.get("FailedItems").and_then(Value::as_array),
        Some(&Vec::new()),
        "a finished execution does not stand in the way of the next one, got {answered}"
    );
    let states = reported("restarted", &emulator, name).await;
    assert_eq!(
        states,
        vec![(
            S3_BUCKET.to_string(),
            "config-bucket".to_string(),
            REMEDIATION_EXECUTION_STATE_QUEUED.to_string(),
        )],
        "the restarted execution is queued again, got {states:?}"
    );
}

#[tokio::test]
async fn a_resource_the_rule_already_found_compliant_is_not_remediated() {
    // "Returns a failure message. For example, the resource is already compliant": a
    // resource the rule's own verdict places as COMPLIANT is a resource it holds no
    // non-compliant evaluation result for, so there is nothing to remediate, no execution
    // is invoked for it and the key comes back.
    let name = "compliant-rule";
    let emulator = emulator_with_rule(name, true).await;

    // The verdicts are delivered the way a rule's evaluation delivers them: through the
    // state the compliance reads report, seeded here because this rule is a managed one
    // whose evaluations Config computes rather than takes from a caller.
    with_verdicts(
        &emulator,
        name,
        "COMPLIANT",
        &[(S3_BUCKET.to_string(), "config-bucket".to_string())],
    );
    found_non_compliant(&emulator, name, &[(EC2_VOLUME, "vol-0001")]);

    let answered = emulator
        .ok(
            "StartRemediationExecution",
            &start(
                name,
                &[(S3_BUCKET, "config-bucket"), (EC2_VOLUME, "vol-0001")],
            ),
        )
        .await;
    let failed = answered["FailedItems"]
        .as_array()
        .unwrap_or_else(|| panic!("expected the compliant key reported back, got {answered}"));
    assert_eq!(
        (failed.len(), failed[0]["resourceId"].as_str()),
        (1, Some("config-bucket")),
        "only the compliant resource is reported back, got {answered}"
    );
    assert!(
        answered["FailureMessage"]
            .as_str()
            .is_some_and(|message| message.contains("no non-compliant EvaluationResult")),
        "the failure message says why nothing ran, got {answered}"
    );

    // The resource that was not found compliant is the one an execution was invoked
    // for.
    let states = reported("compliant resource", &emulator, name).await;
    assert_eq!(
        states,
        vec![(
            EC2_VOLUME.to_string(),
            "vol-0001".to_string(),
            REMEDIATION_EXECUTION_STATE_QUEUED.to_string(),
        )],
        "expected only the noncompliant resource to be remediated, got {states:?}"
    );
}
