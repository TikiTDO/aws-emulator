// SPDX-License-Identifier: CC0-1.0

//! The conformance pack surface, end to end over the wire.
//!
//! The rule pinned here: `PutConformancePack` deploys a conformance pack from the
//! template the request carries, and refuses a `TemplateBody` that is not a conformance
//! pack template — one that is no YAML (or JSON) document, or that declares anything
//! other than the two resource types a conformance pack deploys — with
//! `ConformancePackTemplateValidationException` (400), leaving no pack behind for that
//! name. A deployment it accepts is `CREATE_IN_PROGRESS` when the call answers and
//! `CREATE_COMPLETE` once the clock has made the deployment due, which is what
//! `DescribeConformancePackStatus` reports.
//!
//! Three further properties are pinned with it: the operation is idempotent, so a second
//! call for the same name redeploys that pack rather than creating another — once that
//! pack's deployment has settled, a call made while it is still in flight finding the
//! pack in use (`ResourceInUseException`); exactly one of `TemplateS3Uri`, `TemplateBody`
//! and `TemplateSSMDocumentDetails` may be specified; and a status read pages over the
//! packs' name ordering.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` already defines.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    CONFIG_RULES_QUOTA, CONFORMANCE_PACKS_QUOTA, CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER,
    CONFORMANCE_PACK_SERVICE_PRINCIPAL, CONFORMANCE_PACK_STACK_NAME_PREFIX,
    CONFORMANCE_PACK_STATE_CREATE_COMPLETE, CONFORMANCE_PACK_STATE_CREATE_FAILED,
    CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS, CONFORMANCE_PACK_TYPE_CUSTOM,
    DESCRIBE_CONFORMANCE_PACK_STATUS_PAGE_SIZE,
};

use crate::config_setup_and_rule_loop::Emulator;
use crate::conformance_pack_type_and_stack_identity::{is_stack_id, settled};

/// Long enough for the clock to have made a pack's deployment due, read from the service
/// itself so the test cannot drift from the span the service owes.
pub(crate) fn past_the_deployment() -> Duration {
    CONFORMANCE_PACK_DEPLOY_SETTLE_AFTER + Duration::from_millis(200)
}

/// The resource types a conformance pack template declares, from this operation's own
/// model doc ("You can use a YAML template with two resource types: Config rule
/// (AWS::Config::ConfigRule) and remediation action
/// (AWS::Config::RemediationConfiguration)").
const CONFIG_RULE_TYPE: &str = "AWS::Config::ConfigRule";
/// See [`CONFIG_RULE_TYPE`].
const REMEDIATION_TYPE: &str = "AWS::Config::RemediationConfiguration";

/// A YAML conformance pack template declaring one Config rule named `rule_name`.
///
/// The `Source` is what CloudFormation requires of an `AWS::Config::ConfigRule`, and
/// `S3_BUCKET_VERSIONING_ENABLED` is a Config managed rule identifier.
pub(crate) fn rule_template(rule_name: &str) -> String {
    format!(
        "Resources:\n  OnlyRule:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
         ConfigRuleName: {rule_name}\n      Source:\n        Owner: AWS\n        \
         SourceIdentifier: S3_BUCKET_VERSIONING_ENABLED\n"
    )
}

/// A YAML conformance pack template declaring one Config rule and the remediation
/// action that remediates it.
///
/// `ConfigRuleName`, `TargetType` and `TargetId` are what CloudFormation requires of an
/// `AWS::Config::RemediationConfiguration`, and `AutomationAssumeRole` is the role an
/// AUTOMATIC remediation runs as, which Config requires of one ("for AUTOMATIC
/// remediation configuration, the only valid assumeRole field value is
/// AutomationAssumeRole and you need to provide a value for AutomationAssumeRole to
/// remediate your resources", `PutRemediationConfigurations`).
fn rule_and_remediation_template(rule_name: &str) -> String {
    format!(
        "{}  TheFix:\n    Type: {REMEDIATION_TYPE}\n    Properties:\n      ConfigRuleName: \
         {rule_name}\n      TargetType: SSM_DOCUMENT\n      TargetId: \
         AWS-EnableS3BucketVersioning\n      Automatic: true\n      Parameters:\n        \
         BucketName:\n          ResourceValue:\n            Value: RESOURCE_ID\n        \
         VersioningState:\n          StaticValue:\n            Values:\n              - \
         Enabled\n        AutomationAssumeRole:\n          StaticValue:\n            \
         Values:\n              - {}\n",
        rule_template(rule_name),
        remediation_role(),
    )
}

/// The IAM role an automatic remediation in these templates runs as, in this account.
fn remediation_role() -> String {
    format!(
        "arn:aws:iam::{}:role/aws-config-remediation",
        emulator_server::emulator_account_id(),
    )
}

/// The `PutConformancePack` body for `name` deployed from `template_body`.
pub(crate) fn pack_from_body(name: &str, template_body: &str) -> Value {
    json!({ "ConformancePackName": name, "TemplateBody": template_body })
}

/// The `DescribeConformancePackStatus` body naming `names` (every pack when empty).
fn status_of(names: &[&str]) -> Value {
    if names.is_empty() {
        json!({})
    } else {
        json!({ "ConformancePackNames": names })
    }
}

/// The status details of a `DescribeConformancePackStatus` response.
fn details(case: &str, body: &Value) -> Vec<Value> {
    body["ConformancePackStatusDetails"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected ConformancePackStatusDetails, got {body}")
        })
        .clone()
}

/// The names of the packs a status response reports, in the order reported.
fn reported_names(case: &str, body: &Value) -> Vec<String> {
    details(case, body)
        .iter()
        .map(|detail| {
            detail["ConformancePackName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a status detail carries a name"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_template_body_that_is_not_a_conformance_pack_template_is_refused() {
    // Rows differ along what the rule generalises over: HOW the body fails to be a
    // conformance pack template — unparsable YAML, a document that is not a mapping,
    // more than one document, an alias, nesting far past what a template reaches, no
    // Resources section, an empty one, a declared resource with no type, and a resource
    // type a conformance pack does not deploy — and along the spelling (YAML or JSON) it
    // fails in.
    let cases: [(&str, String); 14] = [
        (
            "the recorded malformed YAML",
            "not: [ valid :: yaml {{".to_string(),
        ),
        (
            "a flow mapping that is never closed",
            "{Resources: {OnlyRule: {Type: AWS::Config::ConfigRule".to_string(),
        ),
        ("a plain scalar document", "just-a-string".to_string()),
        ("a sequence document", "- Resources\n- OnlyRule\n".to_string()),
        (
            "two documents in one body",
            format!(
                "{}---\n{}",
                rule_template("only-rule"),
                rule_template("only-rule")
            ),
        ),
        (
            "a body whose nodes are reached through an alias",
            "Resources: &rule\n  OnlyRule:\n    Type: AWS::Config::ConfigRule\nMore: *rule\n"
                .to_string(),
        ),
        (
            "a body nesting flow collections far past any template",
            format!("Resources: {}", "[".repeat(1_000)),
        ),
        (
            "a body whose block nesting descends on one line",
            format!("Resources:\n{}x\n", "- ".repeat(1_000)),
        ),
        (
            "a template declaring no Resources section",
            "Description: a pack with nothing in it\n".to_string(),
        ),
        (
            "a template whose Resources section is empty",
            "Resources: {}\n".to_string(),
        ),
        (
            "a declared resource carrying no Type",
            "Resources:\n  OnlyRule:\n    Properties:\n      ConfigRuleName: only-rule\n"
                .to_string(),
        ),
        (
            "a resource type a conformance pack does not deploy, in JSON",
            json!({ "Resources": { "Bucket": { "Type": "AWS::S3::Bucket" } } }).to_string(),
        ),
        (
            "a Config rule declaring no Source, which CloudFormation requires",
            format!(
                "Resources:\n  OnlyRule:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
                 ConfigRuleName: only-rule\n"
            ),
        ),
        (
            "a remediation action declaring none of the properties it must",
            format!("Resources:\n  TheFix:\n    Type: {REMEDIATION_TYPE}\n    Properties: {{}}\n"),
        ),
    ];

    let emulator = Emulator::new();
    for (case, template_body) in cases {
        let name = "lifecycle-pack";
        let (status, code) = emulator
            .refused("PutConformancePack", &pack_from_body(name, &template_body))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ConformancePackTemplateValidationException"),
            "case {case:?}: expected 400 ConformancePackTemplateValidationException, got \
             {status} {code}"
        );

        // The refusal leaves NO pack behind for that name.
        let body = emulator
            .ok("DescribeConformancePackStatus", &status_of(&[name]))
            .await;
        assert!(
            details(case, &body).is_empty(),
            "case {case:?}: a refused template must leave no pack behind, got {body}"
        );
    }
}

#[tokio::test]
async fn a_conformance_pack_template_deploys_a_pack_that_reaches_create_complete() {
    // Rows differ along the template the pack is deployed from and what it declares: one
    // Config rule, a rule with the remediation action that remediates it, two rules, the
    // JSON spelling of the same template, and a template carrying the other sections a
    // CloudFormation template may have. The third element of each row is the rule names
    // the template declares, which the deployment must create.
    let cases: [(&str, &str, String, &[&str]); 5] = [
        (
            "a Config rule in YAML",
            "rules-pack",
            rule_template("only-rule"),
            &["only-rule"],
        ),
        (
            "a rule and the remediation action for it",
            "remediation-pack",
            rule_and_remediation_template("fixed-rule"),
            &["fixed-rule"],
        ),
        (
            "two Config rules in one template",
            "two-rules-pack",
            format!(
                "{}  SecondRule:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
                 ConfigRuleName: second-rule\n      Source:\n        Owner: AWS\n        \
                 SourceIdentifier: IAM_PASSWORD_POLICY\n",
                rule_template("first-rule")
            ),
            &["first-rule", "second-rule"],
        ),
        (
            "a Config rule in JSON",
            "json-pack",
            json!({
                "Resources": {
                    "OnlyRule": {
                        "Type": CONFIG_RULE_TYPE,
                        "Properties": {
                            "ConfigRuleName": "json-rule",
                            "Source": {
                                "Owner": "AWS",
                                "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED",
                            },
                        },
                    },
                },
            })
            .to_string(),
            &["json-rule"],
        ),
        (
            "a template carrying other template sections, and a rule it does not name",
            "described-pack",
            [
                "AWSTemplateFormatVersion: '2010-09-09'",
                "Description: a pack",
                "Parameters:",
                "  Who:",
                "    Type: String",
                "Resources:",
                "  UnnamedRule:",
                &format!("    Type: {CONFIG_RULE_TYPE}"),
                "    Properties:",
                "      Source:",
                "        Owner: AWS",
                "        SourceIdentifier: CLOUD_TRAIL_ENABLED",
                "",
            ]
            .join("\n"),
            // A rule the template does not name is named by its logical id, the way
            // CloudFormation names a resource whose name it is not given.
            &["UnnamedRule"],
        ),
    ];

    let emulator = Emulator::new();
    for (case, name, template_body, declared_rules) in cases {
        let created = emulator
            .ok("PutConformancePack", &pack_from_body(name, &template_body))
            .await;
        let arn = created["ConformancePackArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ConformancePackArn, got {created}"))
            .to_string();
        let expected_prefix =
            emulator_server::emulator_arn("config", &emulator.region, "conformance-pack/");
        assert!(
            arn.starts_with(&expected_prefix) && arn.contains(&format!("/{name}/")),
            "case {case:?}: expected the pack ARN to start with {expected_prefix:?} and carry \
             the pack name, got {arn:?}"
        );

        // Straight after the call the deployment is still in progress, and it reports no
        // instant at which it last completed.
        let body = emulator
            .ok("DescribeConformancePackStatus", &status_of(&[name]))
            .await;
        let reported = details(case, &body);
        let [detail] = reported.as_slice() else {
            panic!("case {case:?}: expected exactly one status detail, got {body}");
        };
        assert_eq!(
            (
                detail["ConformancePackState"].as_str(),
                detail["ConformancePackArn"].as_str(),
                detail["LastUpdateCompletedTime"].as_str(),
            ),
            (
                Some(CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS),
                Some(arn.as_str()),
                None,
            ),
            "case {case:?}: a deployment in flight is {CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS} \
             and has not completed, got {detail}"
        );
        // The stack the pack is deployed as is named the way Config names it — the pack's
        // name AND the pack id inside the conformance pack stack-name prefix — and the
        // last ARN segment is CloudFormation's own stack id, a uuid, not anything the
        // pack already carries.
        let pack_id = detail["ConformancePackId"].as_str().unwrap_or_default();
        let expected_stack = emulator_server::emulator_arn(
            "cloudformation",
            &emulator.region,
            &format!("stack/{CONFORMANCE_PACK_STACK_NAME_PREFIX}{name}-{pack_id}/"),
        );
        let stack_arn = detail["StackArn"].as_str().unwrap_or_default();
        assert!(
            !pack_id.is_empty() && stack_arn.starts_with(&expected_stack),
            "case {case:?}: expected the stack ARN to start with {expected_stack:?}, got {detail}"
        );
        assert!(
            is_stack_id(&stack_arn[expected_stack.len().min(stack_arn.len())..]),
            "case {case:?}: expected a uuid stack id as the last ARN segment, got {detail}"
        );
        assert_eq!(
            detail["Type"].as_str(),
            Some(CONFORMANCE_PACK_TYPE_CUSTOM),
            "case {case:?}: a pack this account deployed is \
             {CONFORMANCE_PACK_TYPE_CUSTOM}, got {detail}"
        );
        assert!(
            detail["LastUpdateRequestedTime"].is_number()
                || detail["LastUpdateRequestedTime"].is_string(),
            "case {case:?}: the status carries the instant the deployment was requested, got \
             {detail}"
        );

        // Once the clock has made the deployment due it is complete, and the instant it
        // completed is reported.
        tokio::time::sleep(past_the_deployment()).await;
        let body = emulator
            .ok("DescribeConformancePackStatus", &status_of(&[name]))
            .await;
        let reported = details(case, &body);
        let [detail] = reported.as_slice() else {
            panic!("case {case:?}: expected exactly one status detail, got {body}");
        };
        assert_eq!(
            detail["ConformancePackState"].as_str(),
            Some(CONFORMANCE_PACK_STATE_CREATE_COMPLETE),
            "case {case:?}: once the deployment is due the pack is \
             {CONFORMANCE_PACK_STATE_CREATE_COMPLETE}, got {detail}"
        );
        assert!(
            !detail["LastUpdateCompletedTime"].is_null(),
            "case {case:?}: a completed deployment reports when it completed, got {detail}"
        );

        // The status read is a report: it must not change what the next read sees.
        let again = emulator
            .ok("DescribeConformancePackStatus", &status_of(&[name]))
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: a second status read must report the same detail, got {again}"
        );

        // A pack reported complete is a pack whose resources EXIST: each Config rule the
        // template declared is one this account holds, under the name Config gives a
        // pack's rule and carrying the pack service's principal as its `CreatedBy`.
        let pack_id = detail["ConformancePackId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the status carries the pack id"));
        let rules = emulator.ok("DescribeConfigRules", &json!({})).await;
        for declared in declared_rules {
            let deployed = format!("{declared}-{pack_id}");
            let rule = rules["ConfigRules"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected ConfigRules, got {rules}"))
                .iter()
                .find(|rule| rule["ConfigRuleName"].as_str() == Some(deployed.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the deployed Config rule {deployed:?}, got \
                         {rules}"
                    )
                });
            assert_eq!(
                (
                    rule["CreatedBy"].as_str(),
                    rule["Source"]["Owner"].as_str(),
                    rule["ConfigRuleState"].as_str(),
                ),
                (Some(CONFORMANCE_PACK_SERVICE_PRINCIPAL), Some("AWS"), Some("ACTIVE")),
                "case {case:?}: the pack's rule is an active managed rule the pack service \
                 created, got {rule}"
            );
            assert!(
                rule["Source"]["SourceIdentifier"]
                    .as_str()
                    .is_some_and(|identifier| !identifier.is_empty()),
                "case {case:?}: the rule carries the source identifier the template declared, \
                 got {rule}"
            );
        }
    }
}

#[tokio::test]
async fn a_remediation_action_the_template_declares_is_deployed_for_the_rule_it_names() {
    // The second resource type a conformance pack template declares is deployed too: the
    // remediation configuration of the rule the template names, with the target and the
    // parameters the template gives it. Rows differ along the parameter form declared
    // (`ResourceValue` and `StaticValue`, both in the one template).
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "PutConformancePack",
            &pack_from_body("remediated-pack", &rule_and_remediation_template("fixed-rule")),
        )
        .await;
    assert!(
        created["ConformancePackArn"].as_str().is_some(),
        "the pack is created, got {created}"
    );

    let status = emulator
        .ok("DescribeConformancePackStatus", &status_of(&["remediated-pack"]))
        .await;
    let pack_id = status["ConformancePackStatusDetails"][0]["ConformancePackId"]
        .as_str()
        .unwrap_or_else(|| panic!("the status carries the pack id, got {status}"))
        .to_string();
    let deployed_rule = format!("fixed-rule-{pack_id}");

    let remediations = emulator
        .ok(
            "DescribeRemediationConfigurations",
            &json!({ "ConfigRuleNames": [deployed_rule] }),
        )
        .await;
    let configuration = &remediations["RemediationConfigurations"][0];
    assert_eq!(
        (
            configuration["ConfigRuleName"].as_str(),
            configuration["TargetType"].as_str(),
            configuration["TargetId"].as_str(),
            configuration["Automatic"].as_bool(),
        ),
        (
            Some(deployed_rule.as_str()),
            Some("SSM_DOCUMENT"),
            Some("AWS-EnableS3BucketVersioning"),
            Some(true),
        ),
        "the remediation action the template declared is deployed for the pack's rule, got \
         {remediations}"
    );
    assert_eq!(
        (
            configuration["Parameters"]["BucketName"]["ResourceValue"]["Value"].as_str(),
            configuration["Parameters"]["VersioningState"]["StaticValue"]["Values"][0].as_str(),
        ),
        (Some("RESOURCE_ID"), Some("Enabled")),
        "both parameter forms the template declared are deployed, got {remediations}"
    );
}

#[tokio::test]
async fn a_template_declaring_a_resource_the_account_cannot_create_fails_the_deployment() {
    // A deployment creates only what this account could create directly, so a template
    // declaring a remediation action `PutRemediationConfigurations` would refuse deploys
    // nothing and the pack settles in CREATE_FAILED with the reason. Rows differ along
    // WHICH of that operation's preconditions the declared action breaks — an automatic
    // remediation with no value for the role it runs as, a parameter selecting both value
    // kinds, a parameter selecting neither — and the last row is the positive control:
    // the same template with the role supplied deploys.
    //
    // The remaining precondition, a target Systems Manager holds no automation document
    // for, cannot be exercised here: this harness serves Config alone, so Systems Manager
    // answers nothing about any document and a target stays unknown rather than missing.
    let remediation = |properties: &str| {
        format!(
            "{}  TheFix:\n    Type: {REMEDIATION_TYPE}\n    Properties:\n      ConfigRuleName: \
             fixed-rule\n      TargetType: SSM_DOCUMENT\n      TargetId: \
             AWS-EnableS3BucketVersioning\n{properties}",
            rule_template("fixed-rule")
        )
    };
    let cases: [(&str, String, bool); 4] = [
        (
            "an automatic remediation with no role to run as",
            remediation("      Automatic: true\n"),
            false,
        ),
        (
            "a parameter selecting both a static and a resource value",
            remediation(
                "      Parameters:\n        BucketName:\n          StaticValue:\n            \
                 Values:\n              - logs\n          ResourceValue:\n            Value: \
                 RESOURCE_ID\n",
            ),
            false,
        ),
        (
            "a parameter selecting neither kind of value",
            remediation("      Parameters:\n        BucketName: {}\n"),
            false,
        ),
        (
            "the same remediation with the role it runs as",
            rule_and_remediation_template("fixed-rule"),
            true,
        ),
    ];

    for (case, template_body, deploys) in cases {
        let emulator = Emulator::new();
        let name = "failing-pack";
        // The deployment is ACCEPTED — it is the deployment that fails, not the request.
        let created = emulator
            .ok("PutConformancePack", &pack_from_body(name, &template_body))
            .await;
        assert!(
            created["ConformancePackArn"].as_str().is_some(),
            "case {case:?}: the pack is created and its deployment then runs, got {created}"
        );

        let detail = settled(case, &emulator, name).await;
        let detail = &detail;
        if deploys {
            assert_eq!(
                detail["ConformancePackState"].as_str(),
                Some(CONFORMANCE_PACK_STATE_CREATE_COMPLETE),
                "case {case:?}: a declared remediation the account can create deploys, got \
                 {detail}"
            );
            assert!(
                detail["ConformancePackStatusReason"].is_null(),
                "case {case:?}: a deployment that succeeded has no failure reason, got {detail}"
            );
            continue;
        }

        assert_eq!(
            detail["ConformancePackState"].as_str(),
            Some(CONFORMANCE_PACK_STATE_CREATE_FAILED),
            "case {case:?}: a deployment the account cannot perform is \
             {CONFORMANCE_PACK_STATE_CREATE_FAILED}, got {detail}"
        );
        assert!(
            detail["ConformancePackStatusReason"]
                .as_str()
                .is_some_and(|reason| reason.contains("fixed-rule")),
            "case {case:?}: the failure names the declared rule whose remediation could not be \
             created, got {detail}"
        );
        assert!(
            detail["LastUpdateCompletedTime"].is_null(),
            "case {case:?}: a deployment that failed never completed, got {detail}"
        );

        // A deployment that failed created NOTHING: neither the rules the template
        // declared nor the remediation it declared for them.
        let rules = emulator.ok("DescribeConfigRules", &json!({})).await;
        assert_eq!(
            rules["ConfigRules"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a failed deployment creates no Config rule, got {rules}"
        );
        let remediations = emulator
            .ok("DescribeRemediationConfigurations", &json!({ "ConfigRuleNames": [] }))
            .await;
        assert_eq!(
            remediations["RemediationConfigurations"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {case:?}: a failed deployment creates no remediation action, got {remediations}"
        );

        // The pack is still the account's to redeploy: the same name with a template the
        // account can create deploys, which is what makes the failure observable rather
        // than terminal.
        emulator
            .ok(
                "PutConformancePack",
                &pack_from_body(name, &rule_and_remediation_template("fixed-rule")),
            )
            .await;
        let redeployed = settled(case, &emulator, name).await;
        assert_eq!(
            redeployed["ConformancePackState"].as_str(),
            Some(CONFORMANCE_PACK_STATE_CREATE_COMPLETE),
            "case {case:?}: a redeploy the account can perform completes, got {redeployed}"
        );
        assert!(
            redeployed["ConformancePackStatusReason"].is_null(),
            "case {case:?}: the reason of the deployment that failed is gone, got {redeployed}"
        );
    }
}

#[tokio::test]
async fn a_remediation_action_for_a_rule_the_template_does_not_declare_is_refused() {
    // "The specified Config rule ... must exist": a remediation action remediates one of
    // the pack's own rules. Rows differ along what the template declares beside it.
    let cases: [(&str, String); 2] = [
        (
            "a remediation action alone",
            format!(
                "Resources:\n  TheFix:\n    Type: {REMEDIATION_TYPE}\n    Properties:\n      \
                 ConfigRuleName: no-such-rule\n      TargetType: SSM_DOCUMENT\n      TargetId: \
                 AWS-EnableS3BucketVersioning\n"
            ),
        ),
        (
            "a remediation action naming another rule than the one declared",
            format!(
                "{}  TheFix:\n    Type: {REMEDIATION_TYPE}\n    Properties:\n      \
                 ConfigRuleName: another-rule\n      TargetType: SSM_DOCUMENT\n      TargetId: \
                 AWS-EnableS3BucketVersioning\n",
                rule_template("declared-rule")
            ),
        ),
    ];

    let emulator = Emulator::new();
    for (case, template_body) in cases {
        let (status, code) = emulator
            .refused(
                "PutConformancePack",
                &pack_from_body("unresolved-pack", &template_body),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ConformancePackTemplateValidationException"),
            "case {case:?}: expected 400 ConformancePackTemplateValidationException, got \
             {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_second_put_for_the_same_name_redeploys_that_pack_rather_than_creating_another() {
    // "PutConformancePack is an idempotent API. Subsequent requests won't create a
    // duplicate resource if one was already created." Rows differ along what the second
    // call changes: nothing, the template it deploys, the delivery bucket, and the tags
    // (which this operation does not update).
    let name = "idempotent-pack";
    let cases: [(&str, Value); 4] = [
        (
            "the same request again",
            pack_from_body(name, &rule_template("only-rule")),
        ),
        (
            "another template, declaring another rule",
            pack_from_body(name, &rule_template("another-rule")),
        ),
        ("a delivery bucket", {
            let mut body = pack_from_body(name, &rule_template("only-rule"));
            body["DeliveryS3Bucket"] = json!("config-templates");
            body
        }),
        ("different tags", {
            let mut body = pack_from_body(name, &rule_template("only-rule"));
            body["Tags"] = json!([{ "Key": "owner", "Value": "someone-else" }]);
            body
        }),
    ];

    let emulator = Emulator::new();
    let mut created = pack_from_body(name, &rule_template("only-rule"));
    created["Tags"] = json!([{ "Key": "owner", "Value": "platform" }]);
    let first = emulator.ok("PutConformancePack", &created).await;
    let arn = first["ConformancePackArn"]
        .as_str()
        .expect("the create answers the pack ARN")
        .to_string();

    for (case, request) in cases {
        // A pack is redeployed once its previous deployment has settled; a call made
        // while that deployment is still in flight finds the pack in use, which
        // `a_put_for_a_pack_whose_deployment_is_still_in_flight_is_refused` pins.
        tokio::time::sleep(past_the_deployment()).await;
        let answered = emulator.ok("PutConformancePack", &request).await;
        assert_eq!(
            answered["ConformancePackArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: a second put answers the ARN of the pack that already exists, got \
             {answered}"
        );
        let body = emulator
            .ok("DescribeConformancePackStatus", &status_of(&[]))
            .await;
        assert_eq!(
            reported_names(case, &body),
            vec![name.to_string()],
            "case {case:?}: the account still holds exactly one pack, got {body}"
        );
        // Tags are added at creation and are not updated by this operation.
        let tags = emulator
            .ok("ListTagsForResource", &json!({ "ResourceArn": arn }))
            .await;
        assert_eq!(
            tags["Tags"],
            json!([{ "Key": "owner", "Value": "platform" }]),
            "case {case:?}: the pack keeps the tags it was created with, got {tags}"
        );
    }
}

#[tokio::test]
async fn a_put_for_a_pack_whose_deployment_is_still_in_flight_is_refused() {
    // "For PutConformancePack and PutOrganizationConformancePack, a conformance pack
    // creation, update, and deletion is in progress. Try your request again later."
    // (`ResourceInUseException`.) Rows differ along what the call made while the first
    // deployment is still in flight asks for: the same request again, another template,
    // a delivery location, and other tags.
    let name = "in-flight-pack";
    let cases: [(&str, Value); 4] = [
        (
            "the same request again",
            pack_from_body(name, &rule_template("only-rule")),
        ),
        (
            "another template, declaring another rule",
            pack_from_body(name, &rule_template("another-rule")),
        ),
        ("a delivery location", {
            let mut body = pack_from_body(name, &rule_template("only-rule"));
            body["DeliveryS3Bucket"] = json!("config-templates");
            body
        }),
        ("other tags", {
            let mut body = pack_from_body(name, &rule_template("only-rule"));
            body["Tags"] = json!([{ "Key": "owner", "Value": "someone-else" }]);
            body
        }),
    ];

    for (case, request) in cases {
        let emulator = Emulator::new();
        let created = emulator
            .ok(
                "PutConformancePack",
                &pack_from_body(name, &rule_template("only-rule")),
            )
            .await;
        let arn = created["ConformancePackArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ConformancePackArn, got {created}"))
            .to_string();

        let (status, code) = emulator.refused("PutConformancePack", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ResourceInUseException"),
            "case {case:?}: a put for a pack whose deployment is in flight is refused 400 \
             ResourceInUseException, got {status} {code}"
        );

        // The refusal left the deployment it refused in flight, and stored nothing of the
        // call it refused: the pack still carries what the deployment in flight started
        // with.
        let body = emulator
            .ok("DescribeConformancePackStatus", &status_of(&[name]))
            .await;
        let reported = details(case, &body);
        let [detail] = reported.as_slice() else {
            panic!("case {case:?}: the account holds exactly one pack of that name, got {body}");
        };
        assert_eq!(
            detail["ConformancePackState"].as_str(),
            Some(CONFORMANCE_PACK_STATE_CREATE_IN_PROGRESS),
            "case {case:?}: the refused call leaves the deployment in flight, got {detail}"
        );
        let described = emulator
            .ok(
                "DescribeConformancePacks",
                &json!({ "ConformancePackNames": [name] }),
            )
            .await;
        assert!(
            described["ConformancePackDetails"][0]["DeliveryS3Bucket"].is_null(),
            "case {case:?}: the refused call stored none of its own configuration, got \
             {described}"
        );

        // Once the clock has made that deployment due, the very same call is served, as
        // the redeploy of the pack that already exists.
        tokio::time::sleep(past_the_deployment()).await;
        let answered = emulator.ok("PutConformancePack", &request).await;
        assert_eq!(
            answered["ConformancePackArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: a pack whose deployment has settled is redeployed under the ARN it \
             was created with, got {answered}"
        );
    }

    // The rule is about a deployment in FLIGHT, not about the pack existing: a deployment
    // that FAILED is finished, so the pack it left behind is redeployed with no wait.
    let emulator = Emulator::new();
    let mut oversized = String::from("Resources:\n");
    for index in 0..=CONFIG_RULES_QUOTA {
        oversized.push_str(&format!(
            "  Rule{index}:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
             ConfigRuleName: rule-{index}\n      Source:\n        Owner: AWS\n        \
             SourceIdentifier: IAM_PASSWORD_POLICY\n"
        ));
    }
    emulator
        .ok(
            "PutConformancePack",
            &pack_from_body("failed-pack", &oversized),
        )
        .await;
    let body = emulator
        .ok("DescribeConformancePackStatus", &status_of(&["failed-pack"]))
        .await;
    assert_eq!(
        body["ConformancePackStatusDetails"][0]["ConformancePackState"].as_str(),
        Some("CREATE_FAILED"),
        "a deployment past the Config rules quota of {CONFIG_RULES_QUOTA} failed, got {body}"
    );
    let redeployed = emulator
        .ok(
            "PutConformancePack",
            &pack_from_body("failed-pack", &rule_template("only-rule")),
        )
        .await;
    assert!(
        redeployed["ConformancePackArn"].as_str().is_some(),
        "a pack whose deployment failed is redeployed rather than reported in use, got \
         {redeployed}"
    );
}

#[tokio::test]
async fn exactly_one_template_source_may_be_specified() {
    // "You must specify only one of the follow parameters: TemplateS3Uri, TemplateBody
    // or TemplateSSMDocumentDetails." Rows differ along how many are specified and which.
    let body = rule_template("only-rule");
    let s3_uri = json!("s3://config-templates/pack.yaml");
    let ssm = json!({ "DocumentName": "config-pack-document" });
    let cases: [(&str, Value); 5] = [
        ("no template source at all", json!({})),
        (
            "a body and an S3 URI",
            json!({ "TemplateBody": body, "TemplateS3Uri": s3_uri }),
        ),
        (
            "a body and an SSM document",
            json!({ "TemplateBody": body, "TemplateSSMDocumentDetails": ssm }),
        ),
        (
            "an S3 URI and an SSM document",
            json!({ "TemplateS3Uri": s3_uri, "TemplateSSMDocumentDetails": ssm }),
        ),
        (
            "all three at once",
            json!({
                "TemplateBody": body,
                "TemplateS3Uri": s3_uri,
                "TemplateSSMDocumentDetails": ssm,
            }),
        ),
    ];

    let emulator = Emulator::new();
    for (case, mut request) in cases {
        request["ConformancePackName"] = json!("sources-pack");
        let (status, code) = emulator.refused("PutConformancePack", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected 400 InvalidParameterValueException, got {status} {code}"
        );
    }

    // A template this emulator would have to FETCH is answered with the scaffold's 501
    // rather than deployed from a template nothing read.
    for (case, mut request) in [
        ("an S3 URI alone", json!({ "TemplateS3Uri": s3_uri })),
        (
            "an SSM document alone",
            json!({ "TemplateSSMDocumentDetails": ssm }),
        ),
    ] {
        request["ConformancePackName"] = json!("sources-pack");
        let (status, code) = emulator.refused("PutConformancePack", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (501, "NotImplemented"),
            "case {case:?}: a template outside the request is answered 501 NotImplemented, got \
             {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_redeploy_replaces_exactly_the_resources_this_packs_own_deployment_created() {
    // A redeploy drops what THIS pack created and nothing else: rows differ along what
    // the second template declares against the first — another rule name, the same rule
    // again, and two rules where there was one.
    let emulator = Emulator::new();
    // Another pack's rule, which no redeploy of the pack under test may touch.
    emulator
        .ok(
            "PutConformancePack",
            &pack_from_body("other-pack", &rule_template("other-rule")),
        )
        .await;
    emulator
        .ok(
            "PutConformancePack",
            &pack_from_body("redeployed-pack", &rule_template("first-rule")),
        )
        .await;

    let pack_id = |body: &Value| -> String {
        body["ConformancePackStatusDetails"][0]["ConformancePackId"]
            .as_str()
            .unwrap_or_else(|| panic!("the status carries the pack id, got {body}"))
            .to_string()
    };
    let status = emulator
        .ok(
            "DescribeConformancePackStatus",
            &status_of(&["redeployed-pack"]),
        )
        .await;
    let id = pack_id(&status);
    let other_status = emulator
        .ok("DescribeConformancePackStatus", &status_of(&["other-pack"]))
        .await;
    let other_id = pack_id(&other_status);

    let rule_names = |body: &Value| -> Vec<String> {
        body["ConfigRules"]
            .as_array()
            .unwrap_or_else(|| panic!("expected ConfigRules, got {body}"))
            .iter()
            .filter_map(|rule| rule["ConfigRuleName"].as_str())
            .map(str::to_string)
            .collect()
    };

    let cases: [(&str, String, Vec<String>); 3] = [
        (
            "another rule name",
            rule_template("second-rule"),
            vec![format!("second-rule-{id}")],
        ),
        (
            "the same rule again",
            rule_template("second-rule"),
            vec![format!("second-rule-{id}")],
        ),
        (
            "two rules where there was one",
            format!(
                "{}  Another:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
                 ConfigRuleName: extra-rule\n      Source:\n        Owner: AWS\n        \
                 SourceIdentifier: IAM_PASSWORD_POLICY\n",
                rule_template("second-rule")
            ),
            vec![format!("extra-rule-{id}"), format!("second-rule-{id}")],
        ),
    ];

    for (case, template_body, expected) in cases {
        // The previous deployment of this pack has to have settled before the next one is
        // accepted; see `a_put_for_a_pack_whose_deployment_is_still_in_flight_is_refused`.
        tokio::time::sleep(past_the_deployment()).await;
        emulator
            .ok(
                "PutConformancePack",
                &pack_from_body("redeployed-pack", &template_body),
            )
            .await;
        let mut deployed = rule_names(&emulator.ok("DescribeConfigRules", &json!({})).await);
        deployed.sort();
        let mut want = expected.clone();
        // The other pack's rule is untouched by any redeploy of this one.
        want.push(format!("other-rule-{other_id}"));
        want.sort();
        assert_eq!(
            deployed, want,
            "case {case:?}: a redeploy replaces exactly this pack's own rules, got {deployed:?}"
        );
    }
}

#[tokio::test]
async fn a_deployment_whose_rules_would_pass_the_config_rules_quota_fails_and_creates_nothing() {
    // The deployment is work the service performs, so when it cannot be performed the
    // pack reports the modelled failure state with the reason rather than a completion.
    // Rows differ along how far past the Config rules quota the template reaches.
    let quota = CONFIG_RULES_QUOTA;
    let cases: [(&str, usize); 2] = [
        ("one rule past the quota", quota + 1),
        ("far past it", quota * 2),
    ];

    for (case, declared) in cases {
        let emulator = Emulator::new();
        let mut template = String::from("Resources:\n");
        for index in 0..declared {
            template.push_str(&format!(
                "  Rule{index}:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
                 ConfigRuleName: rule-{index}\n      Source:\n        Owner: AWS\n        \
                 SourceIdentifier: IAM_PASSWORD_POLICY\n"
            ));
        }

        // The pack itself is created — the template is a valid conformance pack template
        // — and its deployment is what failed.
        let created = emulator
            .ok("PutConformancePack", &pack_from_body("big-pack", &template))
            .await;
        assert!(
            created["ConformancePackArn"].as_str().is_some(),
            "case {case:?}: the pack is created, got {created}"
        );

        let body = emulator
            .ok("DescribeConformancePackStatus", &status_of(&["big-pack"]))
            .await;
        let detail = &body["ConformancePackStatusDetails"][0];
        assert_eq!(
            detail["ConformancePackState"].as_str(),
            Some("CREATE_FAILED"),
            "case {case:?}: a deployment that cannot be performed is CREATE_FAILED, got {body}"
        );
        assert!(
            detail["ConformancePackStatusReason"]
                .as_str()
                .is_some_and(|reason| reason.contains(&quota.to_string()))
                && detail["LastUpdateCompletedTime"].is_null(),
            "case {case:?}: a failed deployment reports why and has not completed, got {detail}"
        );

        // Nothing was created: the account holds no rule of that deployment.
        let rules = emulator.ok("DescribeConfigRules", &json!({})).await;
        assert_eq!(
            rules["ConfigRules"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a failed deployment creates no Config rule, got {rules}"
        );

        // Even past the clock-owed span the failed deployment does not become complete.
        tokio::time::sleep(past_the_deployment()).await;
        let body = emulator
            .ok("DescribeConformancePackStatus", &status_of(&["big-pack"]))
            .await;
        assert_eq!(
            body["ConformancePackStatusDetails"][0]["ConformancePackState"].as_str(),
            Some("CREATE_FAILED"),
            "case {case:?}: a failed deployment stays failed, got {body}"
        );
    }
}

#[tokio::test]
async fn a_new_pack_past_this_regions_conformance_pack_quota_is_refused() {
    // Rows differ along where the request sits against the quota: packs created up to
    // one short of it and at it are served, a NEW pack past it is refused, and a pack
    // that already exists is still redeployed past the quota because it adds none.
    let emulator = Emulator::new();
    let template = rule_template("only-rule");

    for index in 0..CONFORMANCE_PACKS_QUOTA {
        let name = format!("pack-{index:04}");
        let created = emulator
            .ok("PutConformancePack", &pack_from_body(&name, &template))
            .await;
        assert!(
            created["ConformancePackArn"].as_str().is_some(),
            "case {name:?}: a pack within the quota of {CONFORMANCE_PACKS_QUOTA} is created, \
             got {created}"
        );
    }

    for (case, name) in [
        ("one pack past the quota", "pack-over-the-quota"),
        ("another new name past it", "pack-also-over"),
    ] {
        let (status, code) = emulator
            .refused("PutConformancePack", &pack_from_body(name, &template))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "MaxNumberOfConformancePacksExceededException"),
            "case {case:?}: a new pack past the quota of {CONFORMANCE_PACKS_QUOTA} is refused \
             400 MaxNumberOfConformancePacksExceededException, got {status} {code}"
        );
    }

    // A pack this account already holds is redeployed rather than created, so the quota
    // does not stand in its way — once that pack's own deployment has settled.
    tokio::time::sleep(past_the_deployment()).await;
    let redeployed = emulator
        .ok("PutConformancePack", &pack_from_body("pack-0000", &template))
        .await;
    assert!(
        redeployed["ConformancePackArn"].as_str().is_some(),
        "an existing pack is redeployed at the quota, got {redeployed}"
    );
}

#[tokio::test]
async fn a_status_read_pages_over_the_packs_this_account_holds() {
    // Rows differ along the page size asked for: one pack per page, several, the whole
    // set at once, and the modelled default of 0 (the maximum).
    let packs = ["alpha", "bravo", "charlie", "delta", "echo"];
    let cases: [(&str, i32, usize); 4] = [
        ("one pack per page", 1, 1),
        ("two packs per page", 2, 2),
        ("more than the account holds", 20, packs.len()),
        (
            "the modelled default",
            0,
            packs.len().min(DESCRIBE_CONFORMANCE_PACK_STATUS_PAGE_SIZE),
        ),
    ];

    let emulator = Emulator::new();
    for name in packs {
        emulator
            .ok(
                "PutConformancePack",
                &pack_from_body(name, &rule_template("only-rule")),
            )
            .await;
    }

    for (case, limit, expected_page) in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({ "Limit": limit });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let body = emulator
                .ok("DescribeConformancePackStatus", &request)
                .await;
            let page = reported_names(case, &body);
            assert!(
                page.len() <= expected_page,
                "case {case:?}: a page carries at most {expected_page} packs, got {page:?}"
            );
            seen.extend(page);
            next_token = body["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen,
            packs.map(str::to_string).to_vec(),
            "case {case:?}: paging reports every pack once, in the packs' name ordering, got \
             {seen:?}"
        );
    }

    // A token that is not in the opaque form this operation issues cannot be read as a
    // page: it is refused rather than taken for a resume point. (A token the operation
    // DID issue is exercised by the paging loop above, which walks the whole set through
    // the tokens it answered.)
    for (case, token) in [
        ("not base64 at all", "**not base64**"),
        (
            "a string outside the token codec's own base64 form",
            "bm90LWEtcGFjaw",
        ),
    ] {
        let (status, code) = emulator
            .refused(
                "DescribeConformancePackStatus",
                &json!({ "NextToken": token }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected 400 InvalidNextTokenException, got {status} {code}"
        );
    }

    // A limit outside the modelled 0..20 range, and a name list past the modelled 25,
    // are refused with the errors this operation models for them.
    for (case, request, expected) in [
        (
            "a limit past the modelled maximum",
            json!({ "Limit": DESCRIBE_CONFORMANCE_PACK_STATUS_PAGE_SIZE + 1 }),
            "InvalidLimitException",
        ),
        (
            "a name list past the modelled maximum",
            json!({ "ConformancePackNames": (0..26).map(|index| format!("pack-{index}")).collect::<Vec<String>>() }),
            "InvalidParameterValueException",
        ),
    ] {
        let (status, code) = emulator
            .refused("DescribeConformancePackStatus", &request)
            .await;
        assert!(
            status == 400 && (code == expected || code == "ValidationException"),
            "case {case:?}: expected 400 {expected} (or the boundary's ValidationException), \
             got {status} {code}"
        );
    }
}
