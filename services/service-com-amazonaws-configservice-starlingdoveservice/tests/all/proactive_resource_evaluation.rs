// SPDX-License-Identifier: CC0-1.0

//! What `GetResourceEvaluationSummary` reports for a proactive resource evaluation,
//! end to end over the wire.
//!
//! `StartResourceEvaluation` and `ListResourceEvaluations` are covered by
//! `resource_evaluations`; this suite is about the SUMMARY, and pins three rules over
//! tables of inputs rather than one recorded case:
//!
//! 1. the summary reports what the start PERSISTED — the mode, the resource details
//!    as the service materialised them, the evaluation context and the instant the
//!    execution started — for any resource type, configuration and context;
//! 2. an execution reports `IN_PROGRESS`, and no compliance, until the instant the
//!    clock owes it a terminal status, and a read never changes what a later read
//!    sees;
//! 3. the terminal status and compliance follow the proactive Config rules the
//!    account holds: an execution the account configured no proactive rule for ends
//!    `FAILED` (nothing ran), one whose covering rules all already placed the resource
//!    SUCCEEDS, one that would have to run a rule's implementation ends `FAILED`
//!    naming that rule (this emulator has no rule engine), and the compliance is
//!    `NOT_APPLICABLE` where no rule verdict applies to the resource and
//!    `INSUFFICIENT_DATA` only where one applies but decided nothing;
//!
//! plus the modelled refusal for an evaluation id this account does not hold.
//!
//! The harness (`Emulator`, `with_recorder_and_channel`) is the one
//! `config_setup_and_rule_loop` already builds.

use std::time::Duration;

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{with_recorder_and_channel, Emulator};

/// The only `EvaluationMode` `StartResourceEvaluation` runs, from
/// `smithy_query(query="shape", shape="EvaluationMode")` (`DETECTIVE` is the other
/// published value).
const PROACTIVE: &str = "PROACTIVE";

/// Every published `ResourceEvaluationStatus` value, from
/// `smithy_query(query="shape", shape="ResourceEvaluationStatus")`: `IN_PROGRESS`,
/// `FAILED`, `SUCCEEDED`.
const IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`IN_PROGRESS`].
const SUCCEEDED: &str = "SUCCEEDED";
/// See [`IN_PROGRESS`].
const FAILED: &str = "FAILED";

/// `ComplianceType` values, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`,
/// `NON_COMPLIANT`, `NOT_APPLICABLE`, `INSUFFICIENT_DATA`. An execution reports
/// `INSUFFICIENT_DATA` when a rule covering the resource decided nothing, and
/// `NOT_APPLICABLE` when no rule verdict applies to it at all.
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";
/// See [`INSUFFICIENT_DATA`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// The only published `ResourceConfigurationSchemaType` value, from
/// `smithy_query(query="shape", shape="ResourceConfigurationSchemaType")`.
const CFN_RESOURCE_SCHEMA: &str = "CFN_RESOURCE_SCHEMA";

/// The modelled `ResourceEvaluationId` length maximum
/// (`com.amazonaws.configservice#ResourceEvaluationId`, length 1..128).
const RESOURCE_EVALUATION_ID_MAX: usize = 128;

/// The `FailureReason` Config answers for an execution the account has no proactive
/// rule for: a fixed sentence carrying no part of the request.
const NO_PROACTIVE_RULE_REASON: &str =
    "No Config rule is enabled in proactive mode for the given resource type.";

/// One row of the round-trip table.
struct SummaryCase {
    name: &'static str,
    resource_type: &'static str,
    resource_id: &'static str,
    configuration: Value,
    schema_type: Option<&'static str>,
    context: Option<&'static str>,
    timeout: i64,
}

/// The `StartResourceEvaluation` body for `case`.
fn start_body(case: &SummaryCase) -> Value {
    let mut details = json!({
        "ResourceId": case.resource_id,
        "ResourceType": case.resource_type,
        "ResourceConfiguration": case.configuration.to_string(),
    });
    if let Some(schema_type) = case.schema_type {
        details["ResourceConfigurationSchemaType"] = json!(schema_type);
    }
    let mut body = json!({
        "ResourceDetails": details,
        "EvaluationMode": PROACTIVE,
        "EvaluationTimeout": case.timeout,
    });
    if let Some(context) = case.context {
        body["EvaluationContext"] = json!({ "EvaluationContextIdentifier": context });
    }
    body
}

/// The resource details the single-purpose cases below evaluate: an S3 bucket
/// definition in the JSON form a CloudFormation resource schema describes.
fn bucket_body() -> Value {
    json!({
        "ResourceDetails": {
            "ResourceId": "config-emulator-bucket",
            "ResourceType": "AWS::S3::Bucket",
            "ResourceConfiguration": json!({
                "BucketName": "config-emulator-bucket",
                "VersioningConfiguration": { "Status": "Enabled" },
            })
            .to_string(),
            "ResourceConfigurationSchemaType": CFN_RESOURCE_SCHEMA,
        },
        "EvaluationMode": PROACTIVE,
        "EvaluationTimeout": 0,
    })
}

/// Start an evaluation and return the id it minted, checked against the modelled
/// length.
async fn start(emulator: &Emulator, case: &str, body: &Value) -> String {
    let started = emulator.ok("StartResourceEvaluation", body).await;
    let id = started["ResourceEvaluationId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a start must answer a ResourceEvaluationId, got {started}")
        })
        .to_string();
    assert!(
        !id.is_empty() && id.len() <= RESOURCE_EVALUATION_ID_MAX,
        "case {case:?}: the id must sit inside the modelled 1..{RESOURCE_EVALUATION_ID_MAX} \
         length, got {id:?}"
    );
    id
}

/// The summary of `id`.
async fn summary(emulator: &Emulator, id: &str) -> Value {
    emulator
        .ok(
            "GetResourceEvaluationSummary",
            &json!({ "ResourceEvaluationId": id }),
        )
        .await
}

#[tokio::test]
async fn a_summary_reports_what_the_start_persisted() {
    let cases = [
        SummaryCase {
            name: "bucket, no context, the documented default timeout",
            resource_type: "AWS::S3::Bucket",
            resource_id: "config-emulator-bucket",
            configuration: json!({ "BucketName": "config-emulator-bucket" }),
            schema_type: None,
            context: None,
            timeout: 0,
        },
        SummaryCase {
            name: "table, a context, an explicit schema type",
            resource_type: "AWS::DynamoDB::Table",
            resource_id: "config-emulator-table",
            configuration: json!({
                "TableName": "config-emulator-table",
                "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            }),
            schema_type: Some(CFN_RESOURCE_SCHEMA),
            context: Some("cfn-stack/config-emulator"),
            timeout: 60,
        },
        SummaryCase {
            name: "role, the published timeout maximum",
            resource_type: "AWS::IAM::Role",
            resource_id: "config-emulator-role",
            configuration: json!({ "RoleName": "config-emulator-role", "MaxSessionDuration": 3600 }),
            schema_type: Some(CFN_RESOURCE_SCHEMA),
            context: Some("cfn-stack/other"),
            // "You cannot specify a number greater than 3600."
            timeout: 3600,
        },
        SummaryCase {
            name: "a third-party resource type, an empty configuration object",
            resource_type: "MyOrg::MyService::MyResource",
            resource_id: "config-emulator-extension-resource",
            configuration: json!({}),
            schema_type: None,
            context: None,
            timeout: 1,
        },
    ];

    let emulator = Emulator::in_region("eu-west-1");
    let mut ids = Vec::new();
    for case in &cases {
        let name = case.name;
        let body = start_body(case);
        let id = start(&emulator, name, &body).await;
        let reported = summary(&emulator, &id).await;

        assert_eq!(
            reported["ResourceEvaluationId"].as_str(),
            Some(id.as_str()),
            "case {name:?}: the summary must report the id it was asked for, got {reported}"
        );
        assert_eq!(
            reported["EvaluationMode"].as_str(),
            Some(PROACTIVE),
            "case {name:?}: the execution's mode must be reported, got {reported}"
        );
        // The service materialises the schema type its definition was read as, so
        // the details are compared against the request's values plus that one.
        let details = &reported["ResourceDetails"];
        for member in ["ResourceId", "ResourceType", "ResourceConfiguration"] {
            assert_eq!(
                details[member], body["ResourceDetails"][member],
                "case {name:?}: the summary must report the {member} the execution \
                 evaluates, got {reported}"
            );
        }
        assert_eq!(
            details["ResourceConfigurationSchemaType"].as_str(),
            Some(case.schema_type.unwrap_or(CFN_RESOURCE_SCHEMA)),
            "case {name:?}: the schema type the definition was read as must be \
             reported, got {reported}"
        );
        assert_eq!(
            reported
                .get("EvaluationContext")
                .and_then(|context| context["EvaluationContextIdentifier"].as_str()),
            case.context,
            "case {name:?}: the summary must report the evaluation context the \
             execution was grouped under, got {reported}"
        );
        let started_at = reported["EvaluationStartTimestamp"]
            .as_f64()
            .unwrap_or_else(|| {
                panic!("case {name:?}: the execution's start must be stamped, got {reported}")
            });
        assert!(
            started_at > 0.0,
            "case {name:?}: the start timestamp must be a real instant, got {started_at}"
        );
        ids.push(id);
    }

    ids.sort();
    let minted = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        minted,
        "every execution must get its own ResourceEvaluationId, got {ids:?}"
    );
}

#[tokio::test]
async fn an_execution_reports_in_progress_until_the_clock_owes_it_a_terminal_status() {
    let emulator = Emulator::new();
    let id = start(&emulator, "just started", &bucket_body()).await;
    let running = summary(&emulator, &id).await;

    assert_eq!(
        running["EvaluationStatus"]["Status"].as_str(),
        Some(IN_PROGRESS),
        "case \"just started\": the execution is still running, got {running}"
    );
    assert!(
        running.get("Compliance").is_none(),
        "case \"just started\": a running execution has placed the resource nowhere \
         yet, got {running}"
    );

    // A read must not move the execution on: a second read while it is still running
    // sees exactly what the first one saw.
    let again = summary(&emulator, &id).await;
    assert_eq!(
        running, again,
        "case \"read twice\": reading the summary must not change what a later read \
         sees, got {again}"
    );

    // The move out of IN_PROGRESS is owed by the clock, not by the read.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let settled = summary(&emulator, &id).await;
    assert_eq!(
        settled["EvaluationStatus"]["Status"].as_str(),
        Some(FAILED),
        "case \"settled\": this account has no proactive rule, so nothing ran, got {settled}"
    );
    assert_eq!(
        settled["Compliance"].as_str(),
        Some(NOT_APPLICABLE),
        "case \"settled\": no rule verdict applies to the resource, got {settled}"
    );
    assert_eq!(
        settled["EvaluationStartTimestamp"], running["EvaluationStartTimestamp"],
        "case \"settled\": the start instant is stamped once, got {settled}"
    );
}

/// One row of the terminal-outcome table: the rule the account holds when the
/// execution runs, the status it ends in, what its failure reason must name (nothing
/// when it did not fail) and where it places the resource.
struct OutcomeCase {
    name: &'static str,
    rule: Option<Value>,
    terminal_status: &'static str,
    reason_names: Option<&'static str>,
    compliance: &'static str,
}

#[tokio::test]
async fn the_terminal_status_follows_the_proactive_rules_the_account_holds() {
    /// A rule named `name` with `extra` members folded into it.
    fn rule(name: &'static str, extra: Value) -> Value {
        let mut config_rule = json!({
            "ConfigRuleName": name,
            "Source": { "Owner": "AWS", "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED" },
        });
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            config_rule[member] = value.clone();
        }
        json!({ "ConfigRule": config_rule })
    }

    let cases = [
        OutcomeCase {
            // Nothing is configured to evaluate in PROACTIVE mode, so there was no
            // execution to run: the evaluation did not run, and no verdict applies.
            name: "no rules at all",
            rule: None,
            terminal_status: FAILED,
            reason_names: Some(NO_PROACTIVE_RULE_REASON),
            compliance: NOT_APPLICABLE,
        },
        OutcomeCase {
            name: "a rule in the default detective mode only",
            rule: Some(rule("detective-only", json!({}))),
            terminal_status: FAILED,
            reason_names: Some(NO_PROACTIVE_RULE_REASON),
            compliance: NOT_APPLICABLE,
        },
        OutcomeCase {
            // The account DOES evaluate proactively, so the execution ran; no rule
            // covers this resource, so nothing placed it.
            name: "a proactive rule scoped to another resource type",
            rule: Some(rule(
                "proactive-elsewhere",
                json!({
                    "Scope": { "ComplianceResourceTypes": ["AWS::DynamoDB::Table"] },
                    "EvaluationModes": [{ "Mode": PROACTIVE }],
                }),
            )),
            terminal_status: SUCCEEDED,
            reason_names: None,
            compliance: NOT_APPLICABLE,
        },
        OutcomeCase {
            name: "a proactive rule scoped to another resource id",
            rule: Some(rule(
                "proactive-other-bucket",
                json!({
                    "Scope": {
                        "ComplianceResourceTypes": ["AWS::S3::Bucket"],
                        "ComplianceResourceId": "some-other-bucket",
                    },
                    "EvaluationModes": [{ "Mode": PROACTIVE }],
                }),
            )),
            terminal_status: SUCCEEDED,
            reason_names: None,
            compliance: NOT_APPLICABLE,
        },
        OutcomeCase {
            // A tag scope narrows by the tags of a RECORDED resource, which the
            // details an execution is handed are not.
            name: "a proactive rule scoped by tag",
            rule: Some(rule(
                "proactive-tagged",
                json!({
                    "Scope": { "TagKey": "owner", "TagValue": "platform" },
                    "EvaluationModes": [{ "Mode": PROACTIVE }],
                }),
            )),
            terminal_status: SUCCEEDED,
            reason_names: None,
            compliance: NOT_APPLICABLE,
        },
        OutcomeCase {
            name: "an unscoped proactive rule",
            rule: Some(rule(
                "proactive-everywhere",
                json!({ "EvaluationModes": [{ "Mode": PROACTIVE }] }),
            )),
            terminal_status: FAILED,
            reason_names: Some("proactive-everywhere"),
            compliance: INSUFFICIENT_DATA,
        },
        OutcomeCase {
            name: "a proactive rule scoped to this resource type",
            rule: Some(rule(
                "proactive-buckets",
                json!({
                    "Scope": { "ComplianceResourceTypes": ["AWS::S3::Bucket"] },
                    "EvaluationModes": [{ "Mode": PROACTIVE }],
                }),
            )),
            terminal_status: FAILED,
            reason_names: Some("proactive-buckets"),
            compliance: INSUFFICIENT_DATA,
        },
        OutcomeCase {
            name: "a proactive rule scoped to this resource type and id",
            rule: Some(rule(
                "proactive-this-bucket",
                json!({
                    "Scope": {
                        "ComplianceResourceTypes": ["AWS::S3::Bucket"],
                        "ComplianceResourceId": "config-emulator-bucket",
                    },
                    "EvaluationModes": [{ "Mode": PROACTIVE }],
                }),
            )),
            terminal_status: FAILED,
            reason_names: Some("proactive-this-bucket"),
            compliance: INSUFFICIENT_DATA,
        },
    ];

    for case in &cases {
        let name = case.name;
        // One account per row: the rules a row puts must not leak into the next.
        let emulator = Emulator::new();
        if let Some(rule) = &case.rule {
            with_recorder_and_channel(&emulator).await;
            emulator.ok("PutConfigRule", rule).await;
        }

        let id = start(&emulator, name, &bucket_body()).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        let reported = summary(&emulator, &id).await;

        let status = &reported["EvaluationStatus"];
        assert_eq!(
            status["Status"].as_str(),
            Some(case.terminal_status),
            "case {name:?}: expected the execution to end {:?}, got {reported}",
            case.terminal_status
        );
        match case.reason_names {
            Some(named) => {
                let reason = status["FailureReason"].as_str().unwrap_or_else(|| {
                    panic!("case {name:?}: a failed execution must say why, got {reported}")
                });
                assert!(
                    reason.contains(named),
                    "case {name:?}: the failure reason must name {named:?}, got {reason:?}"
                );
            }
            None => assert!(
                status.get("FailureReason").is_none(),
                "case {name:?}: an execution that did not fail carries no failure \
                 reason, got {reported}"
            ),
        }
        assert_eq!(
            reported["Compliance"].as_str(),
            Some(case.compliance),
            "case {name:?}: expected Compliance {:?}, got {reported}",
            case.compliance
        );
    }
}

#[tokio::test]
async fn a_summary_of_an_unknown_evaluation_is_refused() {
    let emulator = Emulator::new();
    // A started execution, so the refusals below are about the id asked for and not
    // about an empty store.
    let known = start(&emulator, "a started execution", &bucket_body()).await;

    let appended = format!("{known}0");
    let cases = [
        "00000000000000000000000000000000",
        "not-a-resource-evaluation-id",
        appended.as_str(),
    ];

    for unknown in cases {
        let body = json!({ "ResourceEvaluationId": unknown });
        let (status, code) = emulator
            .refused("GetResourceEvaluationSummary", &body)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceNotFoundException"),
            "case {unknown:?}: an unknown evaluation id must be refused, got {status} {code}"
        );

        let (_, parsed) = emulator.call("GetResourceEvaluationSummary", &body).await;
        let message = parsed["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(unknown),
            "case {unknown:?}: the refusal must name the id the request carried, got {parsed}"
        );
    }

    // The execution that does exist is still readable.
    let reported = summary(&emulator, &known).await;
    assert_eq!(
        reported["ResourceEvaluationId"].as_str(),
        Some(known.as_str()),
        "case \"a started execution\": a refusal must not disturb the execution that \
         exists, got {reported}"
    );
}
