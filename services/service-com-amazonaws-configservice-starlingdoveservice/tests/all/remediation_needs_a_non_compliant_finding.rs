// SPDX-License-Identifier: CC0-1.0

//! What the remediation surface does with a resource the rule has NOT found
//! non-compliant, end to end over the wire.
//!
//! The rules pinned here:
//!
//! 1. `StartRemediationExecution` remediates a FINDING, so it starts an execution only
//!    for a resource key the named rule holds a `NON_COMPLIANT` evaluation result for;
//!    every other key comes back under `FailedItems` with the `FailureMessage` saying
//!    there is no non-compliant evaluation result to remediate, and
//!    `DescribeRemediationExecutionStatus` then reports no execution for it, because none
//!    was started.
//! 2. `PutRemediationExceptions` resolves each resource key against the resources the
//!    named rule has EVALUATED, so a key naming a resource that rule holds no evaluation
//!    result for resolves to no resource and the whole request is refused 400
//!    `InvalidParameterValueException` naming the keys it could not find; nothing is
//!    stored, so `DescribeRemediationExceptions` reports nothing and
//!    `DeleteRemediationExceptions` refuses 400 `NoSuchRemediationExceptionException`.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines — over a TABLE of inputs
//! rather than one recorded call: the rows differ along which verdict the rule holds
//! about the resource (each `ComplianceType` a verdict can carry, no verdict at all, and
//! a verdict about another resource or under another rule), along the resource type and
//! id each key names, and along how many keys one request carries and how many of them
//! resolve.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{
    managed_rule, with_recorder_and_channel, with_verdicts, Emulator,
};

/// Every `ComplianceType` a rule's own evaluation result can carry, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`, `NON_COMPLIANT`,
/// `NOT_APPLICABLE` and `INSUFFICIENT_DATA`.
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`NON_COMPLIANT`].
const COMPLIANT: &str = "COMPLIANT";
/// See [`NON_COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// Resource types a key names, from `smithy_query(query="shape", shape="ResourceType")`;
/// `AWS::::Account` is the account-level type Config uses for the account itself
/// (`ExternalEvaluation$ComplianceResourceType`), which that value set does not list.
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`S3_BUCKET`].
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`S3_BUCKET`].
const ACCOUNT: &str = "AWS::::Account";

/// The rule every case remediates under.
const RULE: &str = "remediated-rule";
/// Another rule, whose findings say nothing about what [`RULE`] can remediate.
const OTHER_RULE: &str = "other-rule";

/// The `FailureMessage` Config returns for a resource it holds no non-compliant
/// evaluation result for; it names no part of the request, so it reads the same for every
/// resource a request carries.
const NO_NON_COMPLIANT_EVALUATION: &str =
    "There are no non-compliant EvaluationResult for the resource(s) that are trying to \
     be remediated. Please wait several minutes and try again when the resource(s) are \
     non-compliant.";

/// The sentence Config words `NoSuchRemediationExceptionException` with.
const NO_SUCH_EXCEPTION: &str = "Specified RemediationException could not be found.";

/// An emulator holding both rules, each with the remediation configuration a remediation
/// and a remediation exception are about (`RemediationConfiguration`: `ConfigRuleName`,
/// `TargetType` and `TargetId` are its required members).
async fn emulator_with_remediated_rules() -> Emulator {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for rule in [RULE, OTHER_RULE] {
        emulator.ok("PutConfigRule", &managed_rule(rule)).await;
        emulator
            .ok(
                "PutRemediationConfigurations",
                &json!({
                    "RemediationConfigurations": [{
                        "ConfigRuleName": rule,
                        "TargetType": "SSM_DOCUMENT",
                        "TargetId": "AWS-PublishSNSNotification",
                    }],
                }),
            )
            .await;
    }
    emulator
}

/// One `(resourceType, resourceId)` pair, spelled the way the tables below want it.
fn resource(resource_type: &str, resource_id: &str) -> (String, String) {
    (resource_type.to_string(), resource_id.to_string())
}

/// One row of the start table: the verdict the rule holds, about which resource, and
/// whether the request the row makes starts an execution for the key it names.
struct StartCase {
    case: &'static str,
    /// The resource the rule has evaluated, the verdict it recorded, and under which
    /// rule; `None` when the rule has evaluated nothing at all.
    found: Option<(&'static str, (&'static str, &'static str), &'static str)>,
    /// The key the request names.
    key: (&'static str, &'static str),
    starts: bool,
}

#[tokio::test]
async fn an_execution_starts_only_for_a_resource_the_rule_finds_non_compliant() {
    let cases: [StartCase; 7] = [
        StartCase {
            case: "the rule finds the bucket non-compliant",
            found: Some((RULE, (S3_BUCKET, "config-bucket"), NON_COMPLIANT)),
            key: (S3_BUCKET, "config-bucket"),
            starts: true,
        },
        StartCase {
            case: "the rule finds the volume non-compliant",
            found: Some((RULE, (EC2_VOLUME, "vol-0001"), NON_COMPLIANT)),
            key: (EC2_VOLUME, "vol-0001"),
            starts: true,
        },
        StartCase {
            case: "the rule finds the account itself non-compliant",
            found: Some((RULE, (ACCOUNT, "000000000000"), NON_COMPLIANT)),
            key: (ACCOUNT, "000000000000"),
            starts: true,
        },
        StartCase {
            case: "the rule has evaluated nothing at all",
            found: None,
            key: (S3_BUCKET, "config-bucket"),
            starts: false,
        },
        StartCase {
            case: "the rule places the bucket as compliant",
            found: Some((RULE, (S3_BUCKET, "config-bucket"), COMPLIANT)),
            key: (S3_BUCKET, "config-bucket"),
            starts: false,
        },
        StartCase {
            case: "the rule does not apply to the bucket",
            found: Some((RULE, (S3_BUCKET, "config-bucket"), NOT_APPLICABLE)),
            key: (S3_BUCKET, "config-bucket"),
            starts: false,
        },
        StartCase {
            case: "another rule is the one that finds the bucket non-compliant",
            found: Some((OTHER_RULE, (S3_BUCKET, "config-bucket"), NON_COMPLIANT)),
            key: (S3_BUCKET, "config-bucket"),
            starts: false,
        },
    ];

    for StartCase {
        case,
        found,
        key,
        starts,
    } in cases
    {
        let emulator = emulator_with_remediated_rules().await;
        if let Some((rule, (resource_type, resource_id), compliance)) = found {
            with_verdicts(
                &emulator,
                rule,
                compliance,
                &[resource(resource_type, resource_id)],
            );
        }

        let answered = emulator
            .ok(
                "StartRemediationExecution",
                &json!({
                    "ConfigRuleName": RULE,
                    "ResourceKeys": [{ "resourceType": key.0, "resourceId": key.1 }],
                }),
            )
            .await;

        // The per-resource outcome: the key that could not start comes back, with the
        // one sentence Config explains it with — and nothing comes back when it started.
        let reported: Vec<(String, String)> = answered["FailedItems"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected FailedItems, got {answered}"))
            .iter()
            .map(|item| {
                (
                    item["resourceType"].as_str().unwrap_or_default().to_string(),
                    item["resourceId"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected_reported: Vec<(String, String)> = if starts {
            Vec::new()
        } else {
            vec![resource(key.0, key.1)]
        };
        assert_eq!(
            reported, expected_reported,
            "case {case:?}: expected FailedItems {expected_reported:?}, got {reported:?} in \
             {answered}"
        );
        let expected_message = (!starts).then_some(NO_NON_COMPLIANT_EVALUATION);
        assert_eq!(
            answered["FailureMessage"].as_str(),
            expected_message,
            "case {case:?}: expected FailureMessage {expected_message:?}, got {answered}"
        );

        // What the report says is what the service did: the read path reports an
        // execution only where one was started.
        let statuses = emulator
            .ok(
                "DescribeRemediationExecutionStatus",
                &json!({ "ConfigRuleName": RULE }),
            )
            .await;
        let started = statuses["RemediationExecutionStatuses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected statuses, got {statuses}"))
            .len();
        assert_eq!(
            started,
            usize::from(starts),
            "case {case:?}: expected {} execution(s) reported, got {statuses}",
            usize::from(starts),
        );
    }
}

/// What a put answers for the keys it names: every one took an exception, the keys came
/// back in `FailedBatches` because the rule does not place them as non-compliant, or the
/// whole request was refused because the keys it lists resolve to no resource the rule has
/// evaluated.
#[derive(Debug, PartialEq, Eq)]
enum PutOutcome {
    Accepted,
    FailedBatch,
    /// The keys the refusal must name, in the order the request carried them.
    Refused(&'static [(&'static str, &'static str)]),
}

/// One row of the exception table: what the rule has evaluated, the keys the put names,
/// and what the put answers.
struct ExceptionCase {
    case: &'static str,
    /// Each verdict the rule holds: under which rule, about which resource, and what it
    /// says.
    found: &'static [(&'static str, (&'static str, &'static str), &'static str)],
    keys: &'static [(&'static str, &'static str)],
    outcome: PutOutcome,
}

/// The exceptions `rule` holds, as `(resourceType, resourceId)` pairs.
async fn held(case: &str, emulator: &Emulator, rule: &str) -> Vec<(String, String)> {
    let body = emulator
        .ok(
            "DescribeRemediationExceptions",
            &json!({ "ConfigRuleName": rule }),
        )
        .await;
    body["RemediationExceptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected RemediationExceptions, got {body}"))
        .iter()
        .map(|exception| {
            (
                exception["ResourceType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                exception["ResourceId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn an_exception_resolves_its_keys_against_the_resources_the_rule_evaluated() {
    let cases: [ExceptionCase; 7] = [
        ExceptionCase {
            case: "one key the rule finds non-compliant",
            found: &[(RULE, (EC2_VOLUME, "vol-0001"), NON_COMPLIANT)],
            keys: &[(EC2_VOLUME, "vol-0001")],
            outcome: PutOutcome::Accepted,
        },
        ExceptionCase {
            case: "three keys of three resource types, all found non-compliant",
            found: &[
                (RULE, (EC2_VOLUME, "vol-0001"), NON_COMPLIANT),
                (RULE, (S3_BUCKET, "reports"), NON_COMPLIANT),
                (RULE, (ACCOUNT, "000000000000"), NON_COMPLIANT),
            ],
            keys: &[
                (EC2_VOLUME, "vol-0001"),
                (S3_BUCKET, "reports"),
                (ACCOUNT, "000000000000"),
            ],
            outcome: PutOutcome::Accepted,
        },
        ExceptionCase {
            case: "the rule has evaluated the resource as compliant",
            found: &[(RULE, (EC2_VOLUME, "vol-0001"), COMPLIANT)],
            keys: &[(EC2_VOLUME, "vol-0001")],
            outcome: PutOutcome::FailedBatch,
        },
        ExceptionCase {
            case: "the rule has evaluated nothing at all",
            found: &[],
            keys: &[(S3_BUCKET, "reports")],
            outcome: PutOutcome::Refused(&[(S3_BUCKET, "reports")]),
        },
        ExceptionCase {
            case: "one evaluated key beside one the rule has never evaluated",
            found: &[(RULE, (EC2_VOLUME, "vol-0001"), NON_COMPLIANT)],
            keys: &[(EC2_VOLUME, "vol-0001"), (EC2_VOLUME, "vol-9999")],
            outcome: PutOutcome::Refused(&[(EC2_VOLUME, "vol-9999")]),
        },
        ExceptionCase {
            case: "two keys the rule has never evaluated",
            found: &[(RULE, (EC2_VOLUME, "vol-0001"), NON_COMPLIANT)],
            keys: &[(S3_BUCKET, "reports"), (ACCOUNT, "000000000000")],
            outcome: PutOutcome::Refused(&[
                (S3_BUCKET, "reports"),
                (ACCOUNT, "000000000000"),
            ]),
        },
        ExceptionCase {
            case: "only another rule has evaluated the resource",
            found: &[(OTHER_RULE, (EC2_VOLUME, "vol-0001"), NON_COMPLIANT)],
            keys: &[(EC2_VOLUME, "vol-0001")],
            outcome: PutOutcome::Refused(&[(EC2_VOLUME, "vol-0001")]),
        },
    ];

    for ExceptionCase {
        case,
        found,
        keys,
        outcome,
    } in cases
    {
        let emulator = emulator_with_remediated_rules().await;
        for (rule, (resource_type, resource_id), compliance) in found {
            with_verdicts(
                &emulator,
                rule,
                compliance,
                &[resource(resource_type, resource_id)],
            );
        }

        let request = json!({
            "ConfigRuleName": RULE,
            "ResourceKeys": keys
                .iter()
                .map(|(resource_type, resource_id)| json!({
                    "ResourceType": resource_type,
                    "ResourceId": resource_id,
                }))
                .collect::<Vec<Value>>(),
            "Message": "waiting on the owning team",
        });

        match outcome {
            PutOutcome::Refused(unresolved) => {
                let (status, parsed) = emulator.call("PutRemediationExceptions", &request).await;
                let code = parsed["__type"]
                    .as_str()
                    .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
                    .unwrap_or("<none>");
                let named: Vec<String> = unresolved
                    .iter()
                    .map(|(resource_type, resource_id)| {
                        format!("({resource_type}, {resource_id})")
                    })
                    .collect();
                let expected = format!("Could not find resources: {RULE}: {}", named.join(", "));
                assert_eq!(
                    (status, code, parsed["message"].as_str()),
                    (400, "InvalidParameterValueException", Some(expected.as_str())),
                    "case {case:?}: expected 400 InvalidParameterValueException {expected:?}, \
                     got {status} {parsed}"
                );
                // A refused put writes NOTHING — not even for the keys that did resolve.
                assert_eq!(
                    held(case, &emulator, RULE).await,
                    Vec::new(),
                    "case {case:?}: a refused put leaves the rule holding no exception"
                );
                // With nothing stored, the delete of those keys removes nothing, which is
                // the refusal that tells a caller its exception was never in force.
                let (status, code) = emulator
                    .refused(
                        "DeleteRemediationExceptions",
                        &json!({
                            "ConfigRuleName": RULE,
                            "ResourceKeys": keys
                                .iter()
                                .map(|(resource_type, resource_id)| json!({
                                    "ResourceType": resource_type,
                                    "ResourceId": resource_id,
                                }))
                                .collect::<Vec<Value>>(),
                        }),
                    )
                    .await;
                assert_eq!(
                    (status, code.as_str()),
                    (400, "NoSuchRemediationExceptionException"),
                    "case {case:?}: expected the delete of an exception nothing holds to be \
                     refused, got {status} {code}"
                );
            }
            PutOutcome::Accepted | PutOutcome::FailedBatch => {
                let answered = emulator.ok("PutRemediationExceptions", &request).await;
                let batches = answered["FailedBatches"]
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected FailedBatches, got {answered}")
                    })
                    .len();
                let accepted = outcome == PutOutcome::Accepted;
                assert_eq!(
                    batches == 0,
                    accepted,
                    "case {case:?}: expected the put to be fully accepted={accepted}, got \
                     {answered}"
                );

                // Read back through the report: the exceptions held are exactly the keys
                // an accepted put named, and none at all where the rule generated none.
                let mut seen = held(case, &emulator, RULE).await;
                seen.sort();
                let mut expected: Vec<(String, String)> = if accepted {
                    keys.iter()
                        .map(|(resource_type, resource_id)| resource(resource_type, resource_id))
                        .collect()
                } else {
                    Vec::new()
                };
                expected.sort();
                assert_eq!(
                    seen, expected,
                    "case {case:?}: expected the exceptions {expected:?} read back, got {seen:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_delete_of_an_exception_the_put_refused_is_refused_with_the_recorded_sentence() {
    // The wording of the refusal is a requirement of its own: it names the resource class
    // rather than echoing the rule or the keys, so it reads the same for every key a
    // caller tries. Rows differ along which keys the delete names and how many.
    let cases: [(&str, &[(&str, &str)]); 3] = [
        ("one key", &[(EC2_VOLUME, "vol-0001")]),
        (
            "two keys of two resource types",
            &[(EC2_VOLUME, "vol-0001"), (S3_BUCKET, "reports")],
        ),
        ("the account-level key", &[(ACCOUNT, "000000000000")]),
    ];

    for (case, keys) in cases {
        let emulator = emulator_with_remediated_rules().await;
        let (status, parsed) = emulator
            .call(
                "DeleteRemediationExceptions",
                &json!({
                    "ConfigRuleName": RULE,
                    "ResourceKeys": keys
                        .iter()
                        .map(|(resource_type, resource_id)| json!({
                            "ResourceType": resource_type,
                            "ResourceId": resource_id,
                        }))
                        .collect::<Vec<Value>>(),
                }),
            )
            .await;
        let code = parsed["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<none>");
        assert_eq!(
            (status, code, parsed["message"].as_str()),
            (
                400,
                "NoSuchRemediationExceptionException",
                Some(NO_SUCH_EXCEPTION),
            ),
            "case {case:?}: expected 400 NoSuchRemediationExceptionException \
             {NO_SUCH_EXCEPTION:?}, got {status} {parsed}"
        );
    }
}
