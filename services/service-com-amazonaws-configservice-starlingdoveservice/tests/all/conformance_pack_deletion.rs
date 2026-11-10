// SPDX-License-Identifier: CC0-1.0

//! What DELETING a conformance pack does, end to end.
//!
//! The rule under test: **`DeleteConformancePack` removes the pack and the resources its
//! deployment created — the Config rules it deployed, the remediation actions deployed for
//! them, and the evaluation results recorded on those rules — so every read of that pack
//! afterwards refuses the name as a pack that does not exist, and the rules it deployed are
//! gone from the account's own rule reads while every other pack's resources are left
//! standing. A name this account holds no pack for is refused
//! `NoSuchConformancePackException`, and a pack whose deployment workflow has not settled
//! is refused `ResourceInUseException` rather than having resources removed from under the
//! work in flight.**
//!
//! The rows differ along what the rule generalises over: how many rules the deleted pack
//! deployed, whether another pack is standing beside it, which read is used to show the
//! pack is gone, and whether the name names a pack at all.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::conformance_pack_rule_name;

use crate::conformance_pack_rule_evaluation::Emulator;

/// The error code every read of a pack this account does not hold answers with.
const NO_SUCH_PACK: &str = "NoSuchConformancePackException";

/// The error code a call that would disturb a workflow in flight answers with.
const IN_USE: &str = "ResourceInUseException";

/// The resource type a conformance pack template declares a Config rule as.
const CONFIG_RULE_TYPE: &str = "AWS::Config::ConfigRule";

/// A managed rule identifier a template declares, from the Config managed-rule list.
const MANAGED_RULE: &str = "S3_BUCKET_VERSIONING_ENABLED";

/// A template declaring one Config rule per name in `declared`.
fn template(declared: &[&str]) -> String {
    let mut body = String::from("Resources:\n");
    for (index, name) in declared.iter().enumerate() {
        body.push_str(&format!(
            "  Rule{index}:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
             ConfigRuleName: {name}\n      Source:\n        Owner: AWS\n        \
             SourceIdentifier: {MANAGED_RULE}\n"
        ));
    }
    body
}

/// `body`'s error code, as the awsJson envelope carries it.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("<no __type in {body}>"))
}

/// Deploy `pack` from a template declaring `declared`, wait for the deployment the clock
/// owes, and answer the names Config gave the deployed rules.
async fn deploy(case: &str, emulator: &Emulator, pack: &str, declared: &[&str]) -> Vec<String> {
    emulator
        .ok(
            "PutConformancePack",
            &json!({ "ConformancePackName": pack, "TemplateBody": template(declared) }),
        )
        .await;
    let status = loop {
        let status = emulator
            .ok(
                "DescribeConformancePackStatus",
                &json!({ "ConformancePackNames": [pack] }),
            )
            .await;
        if status["ConformancePackStatusDetails"][0]["ConformancePackState"]
            == json!("CREATE_COMPLETE")
        {
            break status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    let id = status["ConformancePackStatusDetails"][0]["ConformancePackId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the pack id, got {status}"))
        .to_string();
    declared
        .iter()
        .map(|name| conformance_pack_rule_name(name, &id))
        .collect()
}

/// Whether the account's rule reads report the rule `rule_name`.
async fn holds_rule(emulator: &Emulator, rule_name: &str) -> bool {
    let (status, _) = emulator
        .call(
            "DescribeConfigRules",
            &json!({ "ConfigRuleNames": [rule_name] }),
        )
        .await;
    status == 200
}

#[tokio::test]
async fn deleting_a_pack_removes_it_and_the_resources_its_deployment_created() {
    // The rows differ along how many rules the deleted pack deployed and whether another
    // pack stands beside it.
    let cases: [(&str, &[&str], &[&str]); 4] = [
        ("a pack declaring one rule", &["only-rule"], &[]),
        (
            "a pack declaring several rules",
            &["first-rule", "second-rule", "third-rule"],
            &[],
        ),
        ("a pack declaring two rules", &["left-rule", "right-rule"], &[]),
        (
            "a pack beside another pack's rules",
            &["mine-rule"],
            &["theirs-rule"],
        ),
    ];

    for (case, declared, beside) in cases {
        let emulator = Emulator::new();
        let deleted_pack = "the-pack";
        let standing_pack = "the-other-pack";
        let deleted_rules = deploy(case, &emulator, deleted_pack, declared).await;
        let standing_rules = if beside.is_empty() {
            Vec::new()
        } else {
            deploy(case, &emulator, standing_pack, beside).await
        };

        emulator
            .ok(
                "DeleteConformancePack",
                &json!({ "ConformancePackName": deleted_pack }),
            )
            .await;

        // Every read of the pack that resolves a name now refuses it: the pack is gone,
        // not emptied. (`DescribeConformancePackStatus` reports a page rather than
        // resolving a name, so it reports no status at all for it.)
        for (read, body) in [
            (
                "DescribeConformancePacks",
                json!({ "ConformancePackNames": [deleted_pack] }),
            ),
            (
                "GetConformancePackComplianceDetails",
                json!({ "ConformancePackName": deleted_pack }),
            ),
            (
                "GetConformancePackComplianceSummary",
                json!({ "ConformancePackNames": [deleted_pack] }),
            ),
            (
                "DescribeConformancePackCompliance",
                json!({ "ConformancePackName": deleted_pack }),
            ),
        ] {
            let (status, answered) = emulator.call(read, &body).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, NO_SUCH_PACK),
                "case {case:?}: expected {read} to refuse the deleted pack, got {status} with \
                 {answered}"
            );
        }
        let status = emulator
            .ok(
                "DescribeConformancePackStatus",
                &json!({ "ConformancePackNames": [deleted_pack] }),
            )
            .await;
        assert_eq!(
            status["ConformancePackStatusDetails"],
            json!([]),
            "case {case:?}: expected no status for the deleted pack, got {status}"
        );

        // The refusal an independent read of the deleted pack answers is AWS's own
        // sentence for a pack that does not exist, carrying the name the request asked
        // about — bracketed on the read that takes a LIST of names, bare on the reads that
        // name one pack.
        let (_, described) = emulator
            .call(
                "DescribeConformancePacks",
                &json!({ "ConformancePackNames": [deleted_pack] }),
            )
            .await;
        assert_eq!(
            described["message"].as_str(),
            Some(
                format!("No such conformance pack exists. Conformance Pack(s): [{deleted_pack}]")
                    .as_str()
            ),
            "case {case:?}: expected the list read's own refusal wording, got {described}"
        );
        let (_, detailed) = emulator
            .call(
                "GetConformancePackComplianceDetails",
                &json!({ "ConformancePackName": deleted_pack }),
            )
            .await;
        assert_eq!(
            detailed["message"].as_str(),
            Some(
                format!("No such conformance pack exists. Conformance Pack(s): {deleted_pack}")
                    .as_str()
            ),
            "case {case:?}: expected the single-pack read's own refusal wording, got {detailed}"
        );

        // The rules the pack deployed went with it, and nothing else did.
        for rule in &deleted_rules {
            assert!(
                !holds_rule(&emulator, rule).await,
                "case {case:?}: expected the deployed rule {rule} to be gone"
            );
        }
        for rule in &standing_rules {
            assert!(
                holds_rule(&emulator, rule).await,
                "case {case:?}: expected another pack's rule {rule} to be left standing"
            );
        }

        // A second delete of the same name names nothing: the pack is already gone.
        let (status, answered) = emulator
            .call(
                "DeleteConformancePack",
                &json!({ "ConformancePackName": deleted_pack }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, NO_SUCH_PACK),
            "case {case:?}: expected a second delete to be refused, got {status} with {answered}"
        );
    }
}

#[tokio::test]
async fn a_name_no_pack_answers_to_and_a_workflow_in_flight_are_refused() {
    let emulator = Emulator::new();

    // A name this account holds no pack for.
    let (status, answered) = emulator
        .call(
            "DeleteConformancePack",
            &json!({ "ConformancePackName": "never-deployed" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, NO_SUCH_PACK),
        "case \"a name no pack answers to\": expected the delete to be refused, got {status} \
         with {answered}"
    );

    // A pack whose deployment is still in flight: the delete is refused while the workflow
    // holds it, and taken once the deployment has settled.
    let pack = "in-flight-pack";
    emulator
        .ok(
            "PutConformancePack",
            &json!({
                "ConformancePackName": pack,
                "TemplateBody": template(&["in-flight-rule"]),
            }),
        )
        .await;
    let (status, answered) = emulator
        .call(
            "DeleteConformancePack",
            &json!({ "ConformancePackName": pack }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, IN_USE),
        "case \"a deployment in flight\": expected the delete to be refused, got {status} with \
         {answered}"
    );
    assert_eq!(
        answered["message"].as_str(),
        Some(
            format!(
                "A workflow is currently in progress for the Conformance Pack: {pack}. Please \
                 try later."
            )
            .as_str()
        ),
        "case \"a deployment in flight\": expected the refusal to name the pack, got {answered}"
    );

    // The pack is still there: a refused delete removes nothing.
    emulator
        .ok(
            "DescribeConformancePacks",
            &json!({ "ConformancePackNames": [pack] }),
        )
        .await;

    loop {
        let (status, _) = emulator
            .call(
                "DeleteConformancePack",
                &json!({ "ConformancePackName": pack }),
            )
            .await;
        if status == 200 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let (status, answered) = emulator
        .call(
            "DescribeConformancePacks",
            &json!({ "ConformancePackNames": [pack] }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, NO_SUCH_PACK),
        "case \"a settled deployment\": expected the pack to be gone once deleted, got {status} \
         with {answered}"
    );
}
