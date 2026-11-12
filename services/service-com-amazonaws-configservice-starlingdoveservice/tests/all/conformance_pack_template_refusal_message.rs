// SPDX-License-Identifier: CC0-1.0

//! How `PutConformancePack` WORDS its refusal of a `TemplateBody` it cannot deploy a
//! conformance pack from.
//!
//! The rule pinned here: `ConformancePackTemplateValidationException` carries the
//! service's fixed text for the reading that refused the body, and no part of the body
//! itself — a body that is not the CloudFormation template a pack is deployed from
//! (unreadable, declaring no resource, or declaring a resource without the type and
//! properties CloudFormation requires) carries the fixed sentence "Template passed in the
//! input parameter is invalid", while a template Config COULD read whose contents its own
//! conformance pack validation found errors in carries the fixed sentence "Invalid
//! conformance pack template provided. Errors found: [", the errors found as the bracketed
//! list they are, and "]. Fix your template and try again." So neither refusal names the
//! logical id or the resource type the template wrote, and a client matching on either
//! sentence sees what the service produces.
//!
//! Rows span what the rule generalises over: WHICH reading refuses the body, the resource
//! type a template declares that a pack does not deploy, how many such resources it
//! declares and where among the supported ones they sit, and the spelling (YAML or JSON)
//! the body is written in.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` defines and the template helpers
//! `conformance_packs` defines.

use serde_json::json;

use crate::config_setup_and_rule_loop::Emulator;
use crate::conformance_packs::{pack_from_body, past_the_deployment, rule_template};

/// The resource types a conformance pack template declares, from `PutConformancePack`'s
/// own model doc ("You can use a YAML template with two resource types: Config rule
/// (AWS::Config::ConfigRule) and remediation action
/// (AWS::Config::RemediationConfiguration)").
const CONFIG_RULE_TYPE: &str = "AWS::Config::ConfigRule";
/// See [`CONFIG_RULE_TYPE`].
const REMEDIATION_TYPE: &str = "AWS::Config::RemediationConfiguration";

/// Resource types a conformance pack does NOT deploy, each a type the CloudFormation
/// resource-type registry publishes (`AWS::<service>::<resource>`) and none of them one
/// of the two above. The recorded refusal was driven by the first of them.
const UNSUPPORTED_TYPES: [&str; 3] = [
    "AWS::S3::Bucket",
    "AWS::EC2::Instance",
    "AWS::DynamoDB::Table",
];

/// The fixed text Config words `ConformancePackTemplateValidationException` with for a
/// body that is not the template a conformance pack is deployed from. RECORDED wording.
const NOT_A_TEMPLATE_MESSAGE: &str = "Template passed in the input parameter is invalid";

/// The fixed text Config words the same exception with for a template it read whose
/// contents its conformance pack validation found `errors` in: the opening sentence, the
/// errors as the bracketed list they are, and the closing sentence. RECORDED wording.
fn errors_found_message(errors: &[&str]) -> String {
    format!(
        "Invalid conformance pack template provided. Errors found: [{}]. Fix your template and \
         try again.",
        errors.join(", ")
    )
}

/// The one error Config's conformance pack validation reports for a template declaring a
/// resource of a type a pack does not deploy. RECORDED wording.
const UNSUPPORTED_RESOURCE_TYPE_ERROR: &str =
    "Unsupported resource type(s) provided in the template";

/// A YAML template declaring one resource of `resource_type` under the logical id
/// `logical_id`, carrying the properties a Config rule declares — so the only thing the
/// template can be refused for is the TYPE it declares.
fn resource_template(logical_id: &str, resource_type: &str) -> String {
    format!(
        "Resources:\n  {logical_id}:\n    Type: {resource_type}\n    Properties:\n      \
         Source:\n        Owner: AWS\n        SourceIdentifier: S3_BUCKET_VERSIONING_ENABLED\n"
    )
}

/// The status, error code and message of a refused `PutConformancePack` call.
async fn refusal(emulator: &Emulator, name: &str, template_body: &str) -> (u16, String, String) {
    let (status, parsed) = emulator
        .call("PutConformancePack", &pack_from_body(name, template_body))
        .await;
    let code = parsed["__type"]
        .as_str()
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("<no __type in {parsed}>"));
    let message = parsed["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected the refusal to carry a message, got {parsed}"))
        .to_string();
    (status, code, message)
}

#[tokio::test]
async fn a_template_declaring_a_type_a_pack_does_not_deploy_is_refused_with_the_errors_found() {
    // Rows differ along the unsupported type declared, how many resources declare one,
    // where they sit among the supported resources, and the spelling the body is written
    // in — the refusal is the SAME fixed text with the same single error in every one,
    // because the error is about the type(s) provided and names none of them. The third
    // element is the body fragments the refusal must not report.
    let two_unsupported = format!(
        "Resources:\n  First:\n    Type: {}\n  Second:\n    Type: {}\n",
        UNSUPPORTED_TYPES[0], UNSUPPORTED_TYPES[2]
    );
    let beside_a_rule = format!(
        "{}  NotAConfigRule:\n    Type: {}\n",
        rule_template("only-rule"),
        UNSUPPORTED_TYPES[0]
    );
    let before_a_rule = format!(
        "{}{}",
        resource_template("AheadOfTheRule", UNSUPPORTED_TYPES[1]),
        "  TheRule:\n    Type: AWS::Config::ConfigRule\n    Properties:\n      Source:\n        \
         Owner: AWS\n        SourceIdentifier: IAM_PASSWORD_POLICY\n"
    );
    let cases: [(&str, String, &[&str]); 7] = [
        (
            "the recorded unsupported type, alone in the template",
            resource_template("NotAConfigRule", UNSUPPORTED_TYPES[0]),
            &["NotAConfigRule", UNSUPPORTED_TYPES[0]],
        ),
        (
            "another published type a pack does not deploy",
            resource_template("Server", UNSUPPORTED_TYPES[1]),
            &["Server", UNSUPPORTED_TYPES[1]],
        ),
        (
            "a third published type a pack does not deploy",
            resource_template("Table", UNSUPPORTED_TYPES[2]),
            &["Table", UNSUPPORTED_TYPES[2]],
        ),
        (
            "two resources of two unsupported types",
            two_unsupported,
            &[
                "First",
                "Second",
                UNSUPPORTED_TYPES[0],
                UNSUPPORTED_TYPES[2],
            ],
        ),
        (
            "an unsupported resource declared after a Config rule",
            beside_a_rule,
            &["NotAConfigRule", UNSUPPORTED_TYPES[0]],
        ),
        (
            "an unsupported resource declared before a Config rule",
            before_a_rule,
            &["AheadOfTheRule", UNSUPPORTED_TYPES[1]],
        ),
        (
            "an unsupported type in the JSON spelling of a template",
            json!({
                "Resources": {
                    "Bucket": { "Type": UNSUPPORTED_TYPES[0] },
                    "TheRule": {
                        "Type": CONFIG_RULE_TYPE,
                        "Properties": {
                            "Source": { "Owner": "AWS", "SourceIdentifier": "CLOUD_TRAIL_ENABLED" },
                        },
                    },
                },
            })
            .to_string(),
            &["Bucket", UNSUPPORTED_TYPES[0]],
        ),
    ];

    let expected = errors_found_message(&[UNSUPPORTED_RESOURCE_TYPE_ERROR]);
    let emulator = Emulator::new();
    for (case, template_body, must_not_report) in cases {
        let (status, code, message) = refusal(&emulator, "errors-found-pack", &template_body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ConformancePackTemplateValidationException"),
            "case {case:?}: expected 400 ConformancePackTemplateValidationException, got \
             {status} {code} with message {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        for fragment in must_not_report {
            assert!(
                !message.contains(fragment),
                "case {case:?}: the refusal must report no part of the template body, but it \
                 carries {fragment:?}: {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_body_that_is_not_a_template_is_refused_with_the_fixed_sentence() {
    // Rows differ along HOW the body fails to be the CloudFormation template a pack is
    // deployed from: the recorded unparsable body, a document that is not a mapping,
    // more than one document, no Resources section, an empty one, a resource declaring no
    // Type or one that is no string, and a declared resource of a SUPPORTED type missing
    // the properties CloudFormation requires of it. The third element is the body
    // fragments the refusal must not report.
    let cases: [(&str, String, &[&str]); 10] = [
        (
            "the recorded malformed YAML",
            "not: [ valid :: yaml {{".to_string(),
            &["yaml"],
        ),
        (
            "a plain scalar document",
            "just-a-string".to_string(),
            &["just-a-string"],
        ),
        (
            "a sequence document",
            "- Resources\n- OnlyRule\n".to_string(),
            &["OnlyRule"],
        ),
        (
            "two documents in one body",
            format!(
                "{}---\n{}",
                rule_template("only-rule"),
                rule_template("only-rule")
            ),
            &["only-rule"],
        ),
        (
            "a template declaring no Resources section",
            "Description: a pack with nothing in it\n".to_string(),
            &["Resources", "a pack with nothing in it"],
        ),
        (
            "a template whose Resources section is empty",
            "Resources: {}\n".to_string(),
            &["Resources"],
        ),
        (
            "a declared resource carrying no Type",
            "Resources:\n  OnlyRule:\n    Properties:\n      ConfigRuleName: only-rule\n"
                .to_string(),
            &["OnlyRule", "only-rule"],
        ),
        (
            "a declared resource whose Type is a mapping rather than a type name",
            format!("Resources:\n  OnlyRule:\n    Type:\n      Ref: {CONFIG_RULE_TYPE}\n"),
            &["OnlyRule", CONFIG_RULE_TYPE],
        ),
        (
            "a Config rule declaring no Source, which CloudFormation requires",
            format!(
                "Resources:\n  OnlyRule:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
                 ConfigRuleName: only-rule\n"
            ),
            &["OnlyRule", CONFIG_RULE_TYPE],
        ),
        (
            "a remediation action declaring none of the properties it must",
            format!("Resources:\n  TheFix:\n    Type: {REMEDIATION_TYPE}\n    Properties: {{}}\n"),
            &["TheFix", REMEDIATION_TYPE],
        ),
    ];

    let emulator = Emulator::new();
    for (case, template_body, must_not_report) in cases {
        let (status, code, message) =
            refusal(&emulator, "not-a-template-pack", &template_body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ConformancePackTemplateValidationException"),
            "case {case:?}: expected 400 ConformancePackTemplateValidationException, got \
             {status} {code} with message {message:?}"
        );
        assert_eq!(
            message, NOT_A_TEMPLATE_MESSAGE,
            "case {case:?}: expected the message {NOT_A_TEMPLATE_MESSAGE:?}, got {message:?}"
        );
        for fragment in must_not_report {
            assert!(
                !message.contains(fragment),
                "case {case:?}: the refusal must report no part of the template body, but it \
                 carries {fragment:?}: {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_template_declaring_only_the_types_a_pack_deploys_still_deploys() {
    // The other side of the same reading: the types a conformance pack DOES deploy are
    // accepted, each on its own and together, so the refusal above is about the type
    // declared and not about reading the template at all. A deployed pack is read back
    // through `DescribeConformancePacks`, which reports what the deployment stored.
    let rule_only = rule_template("accepted-rule");
    let rule_and_remediation = format!(
        "{}  TheFix:\n    Type: {REMEDIATION_TYPE}\n    Properties:\n      ConfigRuleName: \
         fixed-rule\n      TargetType: SSM_DOCUMENT\n      TargetId: \
         AWS-EnableS3BucketVersioning\n",
        rule_template("fixed-rule")
    );
    let cases: [(&str, &str, String); 2] = [
        ("only the Config rule type", "accepted-rule-pack", rule_only),
        (
            "both types a conformance pack deploys",
            "accepted-both-pack",
            rule_and_remediation,
        ),
    ];

    let emulator = Emulator::new();
    for (case, name, template_body) in cases {
        let created = emulator
            .ok("PutConformancePack", &pack_from_body(name, &template_body))
            .await;
        assert!(
            created["ConformancePackArn"].as_str().is_some(),
            "case {case:?}: expected the deployment to answer a ConformancePackArn, got {created}"
        );
    }

    tokio::time::sleep(past_the_deployment()).await;
    let described = emulator.ok("DescribeConformancePacks", &json!({})).await;
    let names: Vec<String> = described["ConformancePackDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("expected ConformancePackDetails, got {described}"))
        .iter()
        .map(|detail| {
            detail["ConformancePackName"]
                .as_str()
                .unwrap_or("<none>")
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "accepted-both-pack".to_string(),
            "accepted-rule-pack".to_string()
        ],
        "every accepted template must leave its pack behind, got {described}"
    );
}

/// The refused calls above leave no pack behind for the name they named, so a refusal is a
/// refusal of the whole request rather than a half-stored deployment.
#[tokio::test]
async fn a_refused_template_leaves_no_pack_behind() {
    let bodies: [(&str, String); 2] = [
        (
            "an unsupported resource type",
            resource_template("NotAConfigRule", UNSUPPORTED_TYPES[0]),
        ),
        (
            "a body that is not a template",
            "not: [ valid :: yaml {{".to_string(),
        ),
    ];

    let emulator = Emulator::new();
    for (case, template_body) in bodies {
        let name = "refused-pack";
        let (status, _, message) = refusal(&emulator, name, &template_body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the template to be refused, got {status} {message:?}"
        );
        let (status, parsed) = emulator
            .call(
                "DescribeConformancePacks",
                &json!({ "ConformancePackNames": [name] }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused template must leave no pack behind, got {status} {parsed}"
        );
    }
}
