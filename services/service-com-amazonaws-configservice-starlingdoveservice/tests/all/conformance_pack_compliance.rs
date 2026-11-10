// SPDX-License-Identifier: CC0-1.0

//! `DescribeConformancePackCompliance`, end to end over the wire.
//!
//! The rule pinned here: a conformance pack's compliance is the compliance of the Config
//! rules THAT PACK deployed — each reported under the name the deployment gave it,
//! carrying the `ComplianceType` that rule's own recorded evaluation results roll up to
//! (`NON_COMPLIANT` if any evaluated resource is noncompliant, `COMPLIANT` if at least
//! one is compliant and none is not, `INSUFFICIENT_DATA` when no result places it either
//! way) — narrowed, never changed, by the filter the request carries, paged over the
//! rules' name ordering, and refusing a pack this account holds none of, a filtered rule
//! name the pack deployed no rule for, a compliance type the report does not hold and a
//! token the operation did not issue.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — over a TABLE of
//! inputs rather than one recorded call, using the `Emulator` harness
//! `config_setup_and_rule_loop` defines and the deployment helpers `conformance_packs`
//! defines. The rows differ along how many verdicts of which compliance type a rule
//! carries, how many rules the pack deployed, which rules and which compliance type the
//! filter selects, and the page size asked for.
//!
//! Recording an evaluation result for a MANAGED rule — which is what a conformance pack
//! template declares — is something Config does itself from the configuration state it
//! recorded, which this emulator does not evaluate, so the rows that need results
//! already recorded seed them into the same shared state a handler reads, through the
//! seed `config_compliance_by_config_rule` already defines. Everything else goes over
//! the wire.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    conformance_pack_rule_name, DESCRIBE_CONFORMANCE_PACK_COMPLIANCE_PAGE_SIZE,
};

use crate::config_compliance_by_config_rule::record_results;
use crate::config_setup_and_rule_loop::Emulator;
use crate::conformance_packs::past_the_deployment;

/// Every published `ComplianceType` value this report can reach, from
/// `smithy_query(query="shape", shape="ConformancePackComplianceType")`: `COMPLIANT`,
/// `NON_COMPLIANT`, `INSUFFICIENT_DATA`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// A `ComplianceType` an evaluation result can carry that no conformance pack compliance
/// type holds (`com.amazonaws.configservice#ComplianceType` has it, so a result can be
/// recorded with it; `ConformancePackComplianceType` does not).
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// The resource type a conformance pack template declares a Config rule as, from
/// `PutConformancePack`'s own model doc.
const CONFIG_RULE_TYPE: &str = "AWS::Config::ConfigRule";

/// How Config words `NoSuchConfigRuleInConformancePackException` for a filtered rule
/// name the pack deployed no rule for: the fixed sentence, carrying the rule name the
/// request sent and the pack it asked about.
fn no_such_rule(rule_name: &str, pack: &str) -> String {
    format!("No rule with name {rule_name} exists in conformance pack {pack}")
}

/// One row of the filter table: the case, the request members merged into the read, and
/// the `(ConfigRuleName, ComplianceType)` pairs that filter is expected to select.
type FilterCase<'a> = (&'a str, Value, Vec<(String, &'a str)>);

/// A YAML conformance pack template declaring one Config rule per name in `declared`.
///
/// `S3_BUCKET_VERSIONING_ENABLED` is a Config managed rule identifier, and `Source` is
/// what CloudFormation requires of an `AWS::Config::ConfigRule`.
///
/// `pub(crate)` because every suite about what a DEPLOYED pack's rules report needs
/// exactly this template (`conformance_pack_compliance_details` is the other one).
pub(crate) fn template(declared: &[&str]) -> String {
    let mut body = String::from("Resources:\n");
    for (index, name) in declared.iter().enumerate() {
        body.push_str(&format!(
            "  Rule{index}:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
             ConfigRuleName: {name}\n      Source:\n        Owner: AWS\n        \
             SourceIdentifier: S3_BUCKET_VERSIONING_ENABLED\n"
        ));
    }
    body
}

/// Deploy the pack `pack` from a template declaring one rule per name in `declared`,
/// wait for the deployment the clock owes, and answer the names Config gave the deployed
/// rules — in the same order as `declared`.
///
/// `pub(crate)` for the same reason as [`template`].
pub(crate) async fn deploy(
    case: &str,
    emulator: &Emulator,
    pack: &str,
    declared: &[&str],
) -> Vec<String> {
    emulator
        .ok(
            "PutConformancePack",
            &json!({ "ConformancePackName": pack, "TemplateBody": template(declared) }),
        )
        .await;
    tokio::time::sleep(past_the_deployment()).await;
    let status = emulator
        .ok(
            "DescribeConformancePackStatus",
            &json!({ "ConformancePackNames": [pack] }),
        )
        .await;
    let id = status["ConformancePackStatusDetails"][0]["ConformancePackId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the pack id, got {status}"))
        .to_string();
    declared
        .iter()
        .map(|name| conformance_pack_rule_name(name, &id))
        .collect()
}

/// What a `DescribeConformancePackCompliance` response reports, as
/// `(ConfigRuleName, ComplianceType)` pairs in the order the response carried them.
fn reported(case: &str, body: &Value) -> Vec<(String, String)> {
    body["ConformancePackRuleComplianceList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected ConformancePackRuleComplianceList, got {body}")
        })
        .iter()
        .map(|entry| {
            let name = entry["ConfigRuleName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an entry names its rule, got {entry}"));
            let compliance = entry["ComplianceType"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: an entry carries a ComplianceType, got {entry}")
            });
            (name.to_string(), compliance.to_string())
        })
        .collect()
}

/// `pairs` as the report orders them: the rules' own name ordering.
fn in_name_order(pairs: &[(String, &str)]) -> Vec<(String, String)> {
    let mut ordered: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, compliance)| (name.clone(), compliance.to_string()))
        .collect();
    ordered.sort();
    ordered
}

/// The `DescribeConformancePackCompliance` request for `pack`, with `extras` merged in.
fn request(pack: &str, extras: Value) -> Value {
    let mut body = json!({ "ConformancePackName": pack });
    if let Some(members) = extras.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

#[tokio::test]
async fn every_rule_of_the_pack_reports_the_compliance_its_results_roll_up_to() {
    // Rows differ along the evaluation results the pack's first rule carries: none at
    // all, one compliant resource, several, one noncompliant, a noncompliant one among
    // compliant ones, and only results that place the rule neither way. The pack's
    // SECOND rule carries no result in any row, so each row also shows the report is per
    // rule rather than per pack.
    let cases: [(&str, &[&str], &str); 7] = [
        ("no result at all", &[], INSUFFICIENT_DATA),
        ("one compliant resource", &[COMPLIANT], COMPLIANT),
        (
            "several compliant resources",
            &[COMPLIANT, COMPLIANT, COMPLIANT],
            COMPLIANT,
        ),
        ("one noncompliant resource", &[NON_COMPLIANT], NON_COMPLIANT),
        (
            "a noncompliant resource among compliant ones",
            &[COMPLIANT, NON_COMPLIANT, COMPLIANT],
            NON_COMPLIANT,
        ),
        (
            "only results that place the rule neither way",
            &[NOT_APPLICABLE, NOT_APPLICABLE],
            INSUFFICIENT_DATA,
        ),
        (
            "a noncompliant resource beside one that places nothing",
            &[NOT_APPLICABLE, NON_COMPLIANT],
            NON_COMPLIANT,
        ),
    ];

    let pack = "compliance-pack";
    for (case, verdicts, expected) in cases {
        let emulator = Emulator::new();
        let rules = deploy(case, &emulator, pack, &["versioning", "encryption"]).await;
        record_results(&emulator, &rules[0], verdicts);

        let body = emulator
            .ok("DescribeConformancePackCompliance", &request(pack, json!({})))
            .await;
        assert_eq!(
            body["ConformancePackName"].as_str(),
            Some(pack),
            "case {case:?}: the report names the pack it is about, got {body}"
        );
        assert_eq!(
            reported(case, &body),
            in_name_order(&[
                (rules[0].clone(), expected),
                (rules[1].clone(), INSUFFICIENT_DATA),
            ]),
            "case {case:?}: expected {expected} for {:?} and {INSUFFICIENT_DATA} for the rule \
             with no result, got {body}",
            rules[0]
        );
        // "Controls for the conformance pack": this template declares none, so none is
        // reported rather than one being invented.
        assert!(
            body["ConformancePackRuleComplianceList"]
                .as_array()
                .is_some_and(|entries| entries.iter().all(|entry| entry["Controls"].is_null())),
            "case {case:?}: a pack deployed from a template declaring no control reports none, \
             got {body}"
        );

        // The pack's report of a rule is the SAME roll-up the rule's own compliance read
        // reports: one rule, one verdict, whichever reader is asked.
        let own = emulator
            .ok(
                "DescribeComplianceByConfigRule",
                &json!({ "ConfigRuleNames": [rules[0].clone()] }),
            )
            .await;
        assert_eq!(
            own["ComplianceByConfigRules"][0]["Compliance"]["ComplianceType"].as_str(),
            Some(expected),
            "case {case:?}: the rule's own compliance read must agree with the pack's, got {own}"
        );

        // The read is a report: it must not change what the next reader sees.
        let again = emulator
            .ok("DescribeConformancePackCompliance", &request(pack, json!({})))
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read must report the same compliance, got {again}"
        );
    }
}

#[tokio::test]
async fn the_filter_narrows_the_rules_the_report_carries() {
    // One pack of three rules, each placed at a different compliance type, read through a
    // TABLE of filters: the rows differ along which rule names the filter selects, which
    // compliance type it selects, both together, and a combination nothing satisfies.
    let pack = "filtered-pack";
    let emulator = Emulator::new();
    let rules = deploy(
        "the shared pack",
        &emulator,
        pack,
        &["alpha", "bravo", "charlie"],
    )
    .await;
    record_results(&emulator, &rules[0], &[COMPLIANT]);
    record_results(&emulator, &rules[1], &[COMPLIANT, NON_COMPLIANT]);
    // `rules[2]` carries no result at all, so it rolls up to INSUFFICIENT_DATA.

    let placed: [(String, &str); 3] = [
        (rules[0].clone(), COMPLIANT),
        (rules[1].clone(), NON_COMPLIANT),
        (rules[2].clone(), INSUFFICIENT_DATA),
    ];

    let cases: [FilterCase; 8] = [
        ("no filter at all", json!({}), placed.to_vec()),
        (
            "one rule name",
            json!({ "Filters": { "ConfigRuleNames": [rules[1].clone()] } }),
            vec![placed[1].clone()],
        ),
        (
            "two rule names, sent out of the rules' order",
            json!({ "Filters": { "ConfigRuleNames": [rules[2].clone(), rules[0].clone()] } }),
            vec![placed[0].clone(), placed[2].clone()],
        ),
        (
            "the same rule name twice",
            json!({ "Filters": { "ConfigRuleNames": [rules[0].clone(), rules[0].clone()] } }),
            vec![placed[0].clone()],
        ),
        (
            "the compliant type",
            json!({ "Filters": { "ComplianceType": COMPLIANT } }),
            vec![placed[0].clone()],
        ),
        (
            "the noncompliant type",
            json!({ "Filters": { "ComplianceType": NON_COMPLIANT } }),
            vec![placed[1].clone()],
        ),
        (
            "a name and the compliance type that name holds",
            json!({
                "Filters": {
                    "ConfigRuleNames": [rules[2].clone()],
                    "ComplianceType": INSUFFICIENT_DATA,
                },
            }),
            vec![placed[2].clone()],
        ),
        (
            "a name and a compliance type it does not hold",
            json!({
                "Filters": {
                    "ConfigRuleNames": [rules[0].clone()],
                    "ComplianceType": NON_COMPLIANT,
                },
            }),
            Vec::new(),
        ),
    ];

    for (case, extras, expected) in cases {
        let body = emulator
            .ok("DescribeConformancePackCompliance", &request(pack, extras))
            .await;
        assert_eq!(
            reported(case, &body),
            in_name_order(&expected),
            "case {case:?}: expected the filter to select {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_request_the_pack_cannot_answer_is_refused() {
    // Two packs, so a rule name that is a real Config rule of ANOTHER pack is still not
    // one this pack deployed.
    let emulator = Emulator::new();
    let mine = deploy("the shared pack", &emulator, "mine", &["alpha", "bravo"]).await;
    let theirs = deploy("the other pack", &emulator, "theirs", &["alpha"]).await;

    // Rows differ along what the request asks for that the pack cannot answer: a pack
    // this account holds none of, a rule name nothing holds, a rule name another pack
    // holds, two unknown names (the refusal names the first of them the request carried),
    // a name differing from a deployed one only in case, a compliance type this report
    // does not hold, and tokens outside the opaque form this operation issues.
    let cases: [(&str, Value, &str, Option<String>); 9] = [
        (
            "a pack this account holds none of",
            request("ghost", json!({})),
            "NoSuchConformancePackException",
            Some("No such conformance pack exists. Conformance Pack(s): ghost".to_string()),
        ),
        (
            "a rule name nothing holds",
            request("mine", json!({ "Filters": { "ConfigRuleNames": ["phantom"] } })),
            "NoSuchConfigRuleInConformancePackException",
            Some(no_such_rule("phantom", "mine")),
        ),
        (
            "a rule name another pack holds",
            request(
                "mine",
                json!({ "Filters": { "ConfigRuleNames": [theirs[0].clone()] } }),
            ),
            "NoSuchConfigRuleInConformancePackException",
            Some(no_such_rule(&theirs[0], "mine")),
        ),
        (
            "an unknown name beside one the pack deployed",
            request(
                "mine",
                json!({ "Filters": { "ConfigRuleNames": [mine[0].clone(), "phantom"] } }),
            ),
            "NoSuchConfigRuleInConformancePackException",
            Some(no_such_rule("phantom", "mine")),
        ),
        (
            "two unknown names",
            request(
                "mine",
                json!({ "Filters": { "ConfigRuleNames": ["phantom", "ghost-rule"] } }),
            ),
            "NoSuchConfigRuleInConformancePackException",
            Some(no_such_rule("phantom", "mine")),
        ),
        (
            "the same two unknown names in the other order",
            request(
                "mine",
                json!({ "Filters": { "ConfigRuleNames": ["ghost-rule", "phantom"] } }),
            ),
            "NoSuchConfigRuleInConformancePackException",
            Some(no_such_rule("ghost-rule", "mine")),
        ),
        (
            "a name differing from a deployed one only in case",
            request(
                "mine",
                json!({ "Filters": { "ConfigRuleNames": [mine[0].to_uppercase()] } }),
            ),
            "NoSuchConfigRuleInConformancePackException",
            Some(no_such_rule(&mine[0].to_uppercase(), "mine")),
        ),
        (
            "a token that is not base64 at all",
            request("mine", json!({ "NextToken": "**not base64**" })),
            "InvalidNextTokenException",
            None,
        ),
        (
            "a token outside the token codec's own base64 form",
            request("mine", json!({ "NextToken": "bm90LWEtcnVsZQ" })),
            "InvalidNextTokenException",
            None,
        ),
    ];

    for (case, body, expected_code, expected_message) in cases {
        let (status, parsed) = emulator
            .call("DescribeConformancePackCompliance", &body)
            .await;
        let code = parsed["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<none>");
        assert_eq!(
            (status, code),
            (400, expected_code),
            "case {case:?}: expected 400 {expected_code}, got {status} {parsed}"
        );
        if let Some(expected_message) = expected_message {
            assert_eq!(
                parsed["message"].as_str(),
                Some(expected_message.as_str()),
                "case {case:?}: expected the refusal to read {expected_message:?}, got {parsed}"
            );
        }
    }

    // `ConformancePackComplianceType`'s modelled enum values and
    // `ConformancePackConfigRuleNames`'s modelled length of at most ten are enforced by
    // the request boundary, BEFORE the handler runs, so what a wire caller meets for
    // either is the boundary's `ValidationException` — the rows below pin that, not the
    // handler's own modelled refusals, which stand for a request that reaches it. Rows
    // differ along which modelled constraint the request breaks: a compliance type the
    // enum does not carry, one differing from a published value only in case, and a
    // filtered name list past its length.
    for (case, extras) in [
        (
            "a compliance type no result places a pack's rule at",
            json!({ "Filters": { "ComplianceType": NOT_APPLICABLE } }),
        ),
        (
            "a compliance type differing from a published value only in case",
            json!({ "Filters": { "ComplianceType": "compliant" } }),
        ),
        (
            "a filtered name list past the modelled maximum",
            json!({
                "Filters": {
                    "ConfigRuleNames":
                        (0..11).map(|index| format!("rule-{index}")).collect::<Vec<String>>(),
                },
            }),
        ),
    ] {
        let (status, code) = emulator
            .refused(
                "DescribeConformancePackCompliance",
                &request("mine", extras),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the boundary's 400 ValidationException, got {status} {code}"
        );
    }

    // A refusal changes nothing: the pack's rules are still reported as they were.
    let body = emulator
        .ok("DescribeConformancePackCompliance", &request("mine", json!({})))
        .await;
    assert_eq!(
        reported("after the refusals", &body)
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<String>>(),
        in_name_order(&[
            (mine[0].clone(), INSUFFICIENT_DATA),
            (mine[1].clone(), INSUFFICIENT_DATA),
        ])
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<String>>(),
        "a refused request must leave the pack's rules as they were, got {body}"
    );
}

#[tokio::test]
async fn the_report_pages_over_the_rules_name_ordering() {
    // Rows differ along the page size asked for: one rule per page, two, exactly as many
    // as the pack holds, more than it holds, and the modelled default of 0 (the
    // maximum).
    let pack = "paged-pack";
    let declared = ["alpha", "bravo", "charlie", "delta", "echo"];
    let emulator = Emulator::new();
    let rules = deploy("the shared pack", &emulator, pack, &declared).await;
    let mut expected: Vec<String> = rules.clone();
    expected.sort();

    let cases: [(&str, i32, usize); 5] = [
        ("one rule per page", 1, 1),
        ("two rules per page", 2, 2),
        ("exactly as many as the pack holds", 5, 5),
        ("more than the pack holds", 100, declared.len()),
        (
            "the modelled default",
            0,
            declared
                .len()
                .min(DESCRIBE_CONFORMANCE_PACK_COMPLIANCE_PAGE_SIZE),
        ),
    ];

    for (case, limit, page_bound) in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut body = request(pack, json!({ "Limit": limit }));
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let answered = emulator
                .ok("DescribeConformancePackCompliance", &body)
                .await;
            let page = reported(case, &answered);
            assert!(
                page.len() <= page_bound,
                "case {case:?}: a page carries at most {page_bound} rules, got {page:?}"
            );
            seen.extend(page.into_iter().map(|(name, _)| name));
            next_token = answered["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging reports every rule of the pack once, in the rules' name \
             ordering, got {seen:?}"
        );
    }

    // `Limit`'s modelled 0..1000 range is enforced by the request boundary, BEFORE the
    // handler runs, so what a wire caller meets for a limit outside it is the boundary's
    // `ValidationException`. Rows differ along which side of the range is broken.
    for (case, limit) in [
        (
            "a limit past the modelled maximum",
            json!(DESCRIBE_CONFORMANCE_PACK_COMPLIANCE_PAGE_SIZE + 1),
        ),
        ("a limit below the modelled minimum", json!(-1)),
    ] {
        let (status, code) = emulator
            .refused(
                "DescribeConformancePackCompliance",
                &request(pack, json!({ "Limit": limit })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the boundary's 400 ValidationException, got {status} {code}"
        );
    }
}
