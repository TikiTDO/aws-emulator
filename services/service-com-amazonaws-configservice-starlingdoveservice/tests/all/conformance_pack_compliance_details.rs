// SPDX-License-Identifier: CC0-1.0

//! What `GetConformancePackComplianceDetails` reports, end to end over the wire.
//!
//! The rule pinned here: a conformance pack's compliance DETAILS are the evaluation
//! results recorded for the Config rules THAT PACK deployed — one entry per recorded
//! result, carrying the rule name the deployment gave it, the resource the result is
//! about, the verdict it recorded, the times it was triggered, invoked and recorded at,
//! and the annotation it carried — narrowed, never changed, by the filter the request
//! carries, paged over the pack's rules in name order and each rule's results in the
//! store's order, and refusing a pack this account holds none of, a filtered rule name
//! the pack deployed no rule for, a filter the operation may not serve and a token it did
//! not issue. A result recorded at a compliance type a pack's compliance does not hold
//! has no value the report can carry, so it is left out rather than reported as another
//! type.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — over a TABLE of
//! inputs rather than one recorded call, using the `Emulator` harness
//! `config_setup_and_rule_loop` defines and the deployment helper
//! `conformance_pack_compliance` defines. The rows differ along which resources of which
//! type were evaluated in which mode at which verdict, how many rules the pack deployed,
//! which rules, verdict, resource type and resource ids the filter selects, and the page
//! size asked for.
//!
//! Recording an evaluation result for a MANAGED rule — which is what a conformance pack
//! template declares — is something Config does itself from the configuration state it
//! recorded, which this emulator does not evaluate, so each case seeds the results into
//! the same shared state a handler reads, through the seed
//! `compliance_details_by_config_rule` already defines. Everything else goes over the
//! wire.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    EvaluationResultEntry, GET_CONFORMANCE_PACK_COMPLIANCE_DETAILS_PAGE_SIZE,
};

use crate::compliance_details_by_config_rule::{normalised, page_sizes, seed, wire};
use crate::config_setup_and_rule_loop::Emulator;
use crate::conformance_pack_compliance::deploy;

/// Every published `ConformancePackComplianceType` value, from
/// `smithy_query(query="shape", shape="ConformancePackComplianceType")`: `COMPLIANT`,
/// `NON_COMPLIANT`, `INSUFFICIENT_DATA`. These are the values one entry of this report
/// can carry.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`]. Reported on a result that carries it, but no filter may name it
/// (`ConformancePackEvaluationFilters$ComplianceType`: "The allowed values are COMPLIANT
/// and NON_COMPLIANT. INSUFFICIENT_DATA is not supported").
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// A `ComplianceType` an evaluation result can be recorded with that no conformance pack
/// compliance type holds (`com.amazonaws.configservice#ComplianceType` carries it, so a
/// result can be recorded with it; `ConformancePackComplianceType` does not).
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// The compliance types an entry of this report can carry, which is what decides whether
/// a recorded result has anything to report at all.
const PACK_COMPLIANCE_TYPES: [&str; 3] = [COMPLIANT, NON_COMPLIANT, INSUFFICIENT_DATA];

/// Every published `EvaluationMode` value, from
/// `smithy_query(query="shape", shape="EvaluationMode")`: `DETECTIVE`, `PROACTIVE`.
const DETECTIVE: &str = "DETECTIVE";
/// See [`DETECTIVE`].
const PROACTIVE: &str = "PROACTIVE";

/// Resource types the seeded results name. `EvaluationResultQualifier$ResourceType`
/// points at the "Resource Type Value" column of *Supported Amazon Web Services resource
/// Types*; these are entries from it, chosen so they do not sort in the order they are
/// seeded in.
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`S3_BUCKET`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";
/// See [`S3_BUCKET`].
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`S3_BUCKET`]: a type no seeded result names, so a filter on it selects nothing.
const IAM_ROLE: &str = "AWS::IAM::Role";

/// The instant the first seeded result was evaluated at. Whole seconds, so the
/// epoch-second timestamps the wire carries compare exactly.
const FIRST_EVALUATED_AT: u64 = 1_700_000_000;

/// What one entry of the report is about: the Config rule that recorded the result, and
/// the result itself.
type Recorded = (String, EvaluationResultEntry);

/// One row of the filter table: the case, the request members merged into the read, and
/// the entries that filter is expected to select.
type FilterCase<'a> = (&'a str, Value, Vec<Value>);

/// The evaluation result of `resource_id` (a `resource_type`) evaluated in `mode` at
/// `FIRST_EVALUATED_AT + offset` seconds and placed at `compliance`.
///
/// The three instants an evaluation carries are deliberately NOT one instant: Config
/// invoked the rule a second after the event that triggered the evaluation and recorded
/// the verdict a second after that, so a report that confuses the three cannot pass.
fn result(
    resource_type: &str,
    resource_id: &str,
    mode: &str,
    compliance: &str,
    offset: u64,
) -> EvaluationResultEntry {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(FIRST_EVALUATED_AT + offset);
    let mut entry = EvaluationResultEntry::new(resource_type, resource_id, mode, compliance, at);
    entry.config_rule_invoked_time = at + Duration::from_secs(1);
    entry.result_recorded_time = at + Duration::from_secs(2);
    entry
}

/// Seed each rule of `seeded` with the results recorded for it, and answer those results
/// flattened into the order the report must carry them: the pack's rules in name order,
/// and each rule's results in the store's (resource type, resource id, evaluation mode)
/// order.
fn seed_rules(emulator: &Emulator, seeded: &[(String, Vec<EvaluationResultEntry>)]) -> Vec<Recorded> {
    let mut recorded: Vec<Recorded> = Vec::new();
    for (rule, results) in seeded {
        seed(emulator, rule, results);
        recorded.extend(results.iter().map(|result| (rule.clone(), result.clone())));
    }
    recorded.sort_by(|left, right| (&left.0, left.1.key()).cmp(&(&right.0, right.1.key())));
    recorded
}

/// The entries of `recorded` a report selects, as the wire objects it must report them
/// as.
///
/// A `ConformancePackEvaluationResult` carries exactly the members an `EvaluationResult`
/// does apart from the `ResultToken` neither read path hands back
/// (`smithy_query(query="shape", shape="ConformancePackEvaluationResult")`), so what the
/// rule's own evaluation-details read must report an entry as is what this one must too.
/// A result recorded outside [`PACK_COMPLIANCE_TYPES`] is left out of every expectation:
/// there is no value this report could carry it under.
fn expected(recorded: &[Recorded], keep: impl Fn(&Recorded) -> bool) -> Vec<Value> {
    recorded
        .iter()
        .filter(|entry| PACK_COMPLIANCE_TYPES.contains(&entry.1.compliance_type.as_str()))
        .filter(|entry| keep(entry))
        .map(|(rule, result)| wire(rule, result))
        .collect()
}

/// The `GetConformancePackComplianceDetails` request for `pack`, with `extras` merged in.
fn request(pack: &str, extras: &Value) -> Value {
    let mut body = json!({ "ConformancePackName": pack });
    if let Some(members) = extras.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// The entries one `GetConformancePackComplianceDetails` response reports, normalised so
/// an epoch-second timestamp compares equal however the serializer wrote it.
fn entries(case: &str, body: &Value) -> Vec<Value> {
    body["ConformancePackRuleEvaluationResults"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected ConformancePackRuleEvaluationResults, got {body}")
        })
        .iter()
        .map(normalised)
        .collect()
}

/// Every page of the report for `pack` with `extras` merged in, walked through its
/// `NextToken`: the pages' contents in order, and the size of each page.
async fn walk(
    case: &str,
    emulator: &Emulator,
    pack: &str,
    extras: &Value,
) -> (Vec<Value>, Vec<usize>) {
    let mut reported: Vec<Value> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..=64 {
        let mut body = request(pack, extras);
        if let Some(token) = next_token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let answered = emulator
            .ok("GetConformancePackComplianceDetails", &body)
            .await;
        assert_eq!(
            answered["ConformancePackName"].as_str(),
            Some(pack),
            "case {case:?}: every page names the pack it is about, got {answered}"
        );
        let page = entries(case, &answered);
        sizes.push(page.len());
        reported.extend(page);
        match answered["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => return (reported, sizes),
        }
    }
    panic!("case {case:?}: the page walk did not finish, got pages {sizes:?}");
}

#[tokio::test]
async fn every_recorded_result_of_the_packs_rules_is_reported() {
    // Rows differ along what the pack's two rules recorded: nothing at all, one result,
    // results on both rules, several resources of several types, the same resource
    // evaluated in both modes, an annotated result, a result no evaluation could place
    // (INSUFFICIENT_DATA, which the report carries) and one recorded at a compliance type
    // a pack's compliance does not hold (NOT_APPLICABLE, which it cannot).
    let mut annotated = result(DYNAMODB_TABLE, "orders", DETECTIVE, NON_COMPLIANT, 3);
    annotated.annotation = Some("point-in-time recovery is disabled".to_string());

    let cases: [(&str, Vec<EvaluationResultEntry>, Vec<EvaluationResultEntry>); 8] = [
        ("no result at all", Vec::new(), Vec::new()),
        (
            "one compliant resource on the first rule",
            vec![result(S3_BUCKET, "logs", DETECTIVE, COMPLIANT, 0)],
            Vec::new(),
        ),
        (
            "a result on each rule",
            vec![result(S3_BUCKET, "logs", DETECTIVE, COMPLIANT, 0)],
            vec![result(EC2_VOLUME, "vol-0ab", DETECTIVE, NON_COMPLIANT, 1)],
        ),
        (
            "several resources of several types",
            vec![
                result(S3_BUCKET, "logs", DETECTIVE, COMPLIANT, 0),
                result(DYNAMODB_TABLE, "orders", DETECTIVE, NON_COMPLIANT, 1),
                result(EC2_VOLUME, "vol-0ab", DETECTIVE, COMPLIANT, 2),
            ],
            Vec::new(),
        ),
        (
            "one resource evaluated in both modes",
            vec![
                result(S3_BUCKET, "logs", DETECTIVE, COMPLIANT, 0),
                result(S3_BUCKET, "logs", PROACTIVE, NON_COMPLIANT, 1),
            ],
            Vec::new(),
        ),
        ("an annotated result", vec![annotated], Vec::new()),
        (
            "a result no evaluation could place",
            vec![result(IAM_ROLE, "service-role", DETECTIVE, INSUFFICIENT_DATA, 0)],
            vec![result(S3_BUCKET, "logs", DETECTIVE, COMPLIANT, 1)],
        ),
        (
            "a result at a compliance type a pack's compliance does not hold",
            vec![
                result(EC2_VOLUME, "vol-0ab", DETECTIVE, NOT_APPLICABLE, 0),
                result(S3_BUCKET, "logs", DETECTIVE, COMPLIANT, 1),
            ],
            Vec::new(),
        ),
    ];

    let pack = "details-pack";
    for (case, first, second) in cases {
        let emulator = Emulator::new();
        let rules = deploy(case, &emulator, pack, &["alpha", "bravo"]).await;
        let recorded = seed_rules(
            &emulator,
            &[(rules[0].clone(), first), (rules[1].clone(), second)],
        );

        let (reported, _) = walk(case, &emulator, pack, &json!({})).await;
        assert_eq!(
            reported,
            expected(&recorded, |_| true),
            "case {case:?}: expected every recorded result of the pack's rules, got \
             {reported:#?}"
        );

        // The entries the pack reports for one of its rules are the results that RULE's
        // own evaluation-details read reports: one record, read two ways.
        let own = emulator
            .ok(
                "GetComplianceDetailsByConfigRule",
                &json!({ "ConfigRuleName": rules[0].clone(), "Limit": 100 }),
            )
            .await;
        let own_entries: Vec<Value> = own["EvaluationResults"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected EvaluationResults, got {own}"))
            .iter()
            .map(normalised)
            .filter(|entry| {
                PACK_COMPLIANCE_TYPES.contains(&entry["ComplianceType"].as_str().unwrap_or_default())
            })
            .collect();
        assert_eq!(
            expected(&recorded, |(rule, _)| rule == &rules[0]),
            own_entries,
            "case {case:?}: the pack's entries for {:?} must be that rule's own recorded \
             results, got {own}",
            rules[0]
        );

        // The read is a report: it must not change what the next reader sees.
        let again = emulator
            .ok(
                "GetConformancePackComplianceDetails",
                &request(pack, &json!({})),
            )
            .await;
        assert_eq!(
            entries(case, &again),
            expected(&recorded, |_| true),
            "case {case:?}: a second read must report the same results, got {again}"
        );
    }
}

#[tokio::test]
async fn the_filter_narrows_the_results_the_report_carries() {
    // One pack of two rules whose recorded results span three resource types, both
    // evaluation modes and every compliance type a result can carry, read through a TABLE
    // of filters: the rows differ along which rule names, which compliance type, which
    // resource type and which resource ids the filter selects, and combinations of them,
    // including one nothing satisfies.
    let pack = "filtered-details-pack";
    let emulator = Emulator::new();
    let rules = deploy("the shared pack", &emulator, pack, &["alpha", "bravo"]).await;
    let recorded = seed_rules(
        &emulator,
        &[
            (
                rules[0].clone(),
                vec![
                    result(S3_BUCKET, "logs", DETECTIVE, COMPLIANT, 0),
                    result(DYNAMODB_TABLE, "orders", DETECTIVE, NON_COMPLIANT, 1),
                    result(EC2_VOLUME, "vol-0ab", PROACTIVE, COMPLIANT, 2),
                ],
            ),
            (
                rules[1].clone(),
                vec![
                    result(S3_BUCKET, "logs", DETECTIVE, NON_COMPLIANT, 3),
                    result(EC2_VOLUME, "vol-0cd", DETECTIVE, INSUFFICIENT_DATA, 4),
                    result(DYNAMODB_TABLE, "orders", PROACTIVE, NOT_APPLICABLE, 5),
                ],
            ),
        ],
    );

    let cases: [FilterCase; 11] = [
        ("no filter at all", json!({}), expected(&recorded, |_| true)),
        (
            "one rule name",
            json!({ "Filters": { "ConfigRuleNames": [rules[1].clone()] } }),
            expected(&recorded, |(rule, _)| rule == &rules[1]),
        ),
        (
            "two rule names, sent out of the rules' order",
            json!({ "Filters": { "ConfigRuleNames": [rules[1].clone(), rules[0].clone()] } }),
            expected(&recorded, |_| true),
        ),
        (
            "the same rule name twice",
            json!({ "Filters": { "ConfigRuleNames": [rules[0].clone(), rules[0].clone()] } }),
            expected(&recorded, |(rule, _)| rule == &rules[0]),
        ),
        (
            "the compliant type",
            json!({ "Filters": { "ComplianceType": COMPLIANT } }),
            expected(&recorded, |(_, result)| result.compliance_type == COMPLIANT),
        ),
        (
            "the noncompliant type",
            json!({ "Filters": { "ComplianceType": NON_COMPLIANT } }),
            expected(&recorded, |(_, result)| {
                result.compliance_type == NON_COMPLIANT
            }),
        ),
        (
            "one resource type",
            json!({ "Filters": { "ResourceType": S3_BUCKET } }),
            expected(&recorded, |(_, result)| result.resource_type == S3_BUCKET),
        ),
        (
            "a resource type and one of its ids",
            json!({ "Filters": { "ResourceType": EC2_VOLUME, "ResourceIds": ["vol-0ab"] } }),
            expected(&recorded, |(_, result)| {
                result.resource_type == EC2_VOLUME && result.resource_id == "vol-0ab"
            }),
        ),
        (
            "a resource type and two ids, one of which nothing holds",
            json!({
                "Filters": { "ResourceType": EC2_VOLUME, "ResourceIds": ["vol-0cd", "vol-ghost"] },
            }),
            expected(&recorded, |(_, result)| {
                result.resource_type == EC2_VOLUME && result.resource_id == "vol-0cd"
            }),
        ),
        (
            "a rule name beside the resource type that rule evaluated",
            json!({
                "Filters": {
                    "ConfigRuleNames": [rules[0].clone()],
                    "ResourceType": DYNAMODB_TABLE,
                },
            }),
            expected(&recorded, |(rule, result)| {
                rule == &rules[0] && result.resource_type == DYNAMODB_TABLE
            }),
        ),
        (
            "a resource type no result names",
            json!({ "Filters": { "ResourceType": IAM_ROLE } }),
            Vec::new(),
        ),
    ];

    for (case, extras, selected) in cases {
        let (reported, _) = walk(case, &emulator, pack, &extras).await;
        assert_eq!(
            reported, selected,
            "case {case:?}: expected the {} entries the filter selects, got {} in \
             {reported:#?}",
            selected.len(),
            reported.len()
        );
    }
}

#[tokio::test]
async fn a_request_the_report_cannot_serve_is_refused() {
    // Two packs, so a rule name that is a real Config rule of ANOTHER pack is still not
    // one this pack deployed.
    let emulator = Emulator::new();
    let mine = deploy("the shared pack", &emulator, "mine", &["alpha", "bravo"]).await;
    let theirs = deploy("the other pack", &emulator, "theirs", &["alpha"]).await;
    seed(
        &emulator,
        &mine[0],
        &[
            result(S3_BUCKET, "logs", DETECTIVE, COMPLIANT, 0),
            result(EC2_VOLUME, "vol-0ab", DETECTIVE, NON_COMPLIANT, 1),
        ],
    );

    // A token the SIBLING report issues, which this one must not accept: a token is
    // opaque to the caller but belongs to the operation that minted it.
    let siblings_token = emulator
        .ok(
            "DescribeConformancePackCompliance",
            &json!({ "ConformancePackName": "mine", "Limit": 1 }),
        )
        .await["NextToken"]
        .as_str()
        .expect("a two-rule pack at Limit 1 leaves a next page")
        .to_string();

    // Rows differ along what the request asks for that the report cannot serve: a pack
    // this account holds none of, a rule name nothing holds, a rule name another pack
    // holds, an unknown name beside a known one, two unknown names (the refusal names the
    // first the request carried), a name differing from a deployed one only in case, a
    // compliance type no filter may name, resource ids sent without the type that
    // completes them, and tokens outside the opaque form this operation issues.
    let cases: [(&str, Value, &str, Option<String>); 12] = [
        (
            "a pack this account holds none of",
            request("ghost", &json!({})),
            "NoSuchConformancePackException",
            Some("No such conformance pack exists. Conformance Pack(s): ghost".to_string()),
        ),
        (
            "a rule name nothing holds",
            request("mine", &json!({ "Filters": { "ConfigRuleNames": ["phantom"] } })),
            "NoSuchConfigRuleInConformancePackException",
            Some("No rule with name phantom exists in conformance pack mine".to_string()),
        ),
        (
            "a rule name another pack holds",
            request(
                "mine",
                &json!({ "Filters": { "ConfigRuleNames": [theirs[0].clone()] } }),
            ),
            "NoSuchConfigRuleInConformancePackException",
            Some(format!(
                "No rule with name {} exists in conformance pack mine",
                theirs[0]
            )),
        ),
        (
            "an unknown name beside one the pack deployed",
            request(
                "mine",
                &json!({ "Filters": { "ConfigRuleNames": [mine[0].clone(), "phantom"] } }),
            ),
            "NoSuchConfigRuleInConformancePackException",
            Some("No rule with name phantom exists in conformance pack mine".to_string()),
        ),
        (
            "two unknown names",
            request(
                "mine",
                &json!({ "Filters": { "ConfigRuleNames": ["phantom", "ghost-rule"] } }),
            ),
            "NoSuchConfigRuleInConformancePackException",
            Some("No rule with name phantom exists in conformance pack mine".to_string()),
        ),
        (
            "the same two unknown names in the other order",
            request(
                "mine",
                &json!({ "Filters": { "ConfigRuleNames": ["ghost-rule", "phantom"] } }),
            ),
            "NoSuchConfigRuleInConformancePackException",
            Some("No rule with name ghost-rule exists in conformance pack mine".to_string()),
        ),
        (
            "a name differing from a deployed one only in case",
            request(
                "mine",
                &json!({ "Filters": { "ConfigRuleNames": [mine[0].to_uppercase()] } }),
            ),
            "NoSuchConfigRuleInConformancePackException",
            Some(format!(
                "No rule with name {} exists in conformance pack mine",
                mine[0].to_uppercase()
            )),
        ),
        (
            "a compliance type no filter may name",
            request(
                "mine",
                &json!({ "Filters": { "ComplianceType": INSUFFICIENT_DATA } }),
            ),
            "InvalidParameterValueException",
            Some(format!(
                "'{INSUFFICIENT_DATA}' is not a valid value for ComplianceType; the valid \
                 values are {COMPLIANT}, {NON_COMPLIANT}."
            )),
        ),
        (
            "resource ids without the resource type that completes them",
            request("mine", &json!({ "Filters": { "ResourceIds": ["logs"] } })),
            "InvalidParameterValueException",
            Some(
                "If you specify a resource ID, you must also specify a type for ResourceType."
                    .to_string(),
            ),
        ),
        (
            "a token that is not base64 at all",
            request("mine", &json!({ "NextToken": "**not base64**" })),
            "InvalidNextTokenException",
            None,
        ),
        (
            "an empty token",
            request("mine", &json!({ "NextToken": "" })),
            "InvalidNextTokenException",
            None,
        ),
        (
            "a token the sibling compliance report issued",
            request("mine", &json!({ "NextToken": siblings_token })),
            "InvalidNextTokenException",
            None,
        ),
    ];

    for (case, body, expected_code, expected_message) in cases {
        let (status, parsed) = emulator
            .call("GetConformancePackComplianceDetails", &body)
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

    // `ConformancePackComplianceType`'s modelled enum values, the modelled lengths of
    // `ConformancePackConfigRuleNames` (0..10) and `ConformancePackComplianceResourceIds`
    // (0..5) and `Limit`'s modelled range (0..100) are enforced by the request boundary,
    // BEFORE the handler runs, so what a wire caller meets for any of them is the
    // boundary's `ValidationException`. Rows differ along which modelled constraint the
    // request breaks.
    for (case, extras) in [
        (
            "a compliance type outside the modelled enum",
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
        (
            "a filtered resource id list past the modelled maximum",
            json!({
                "Filters": {
                    "ResourceType": S3_BUCKET,
                    "ResourceIds":
                        (0..6).map(|index| format!("res-{index}")).collect::<Vec<String>>(),
                },
            }),
        ),
        (
            "a limit past the modelled maximum",
            json!({ "Limit": GET_CONFORMANCE_PACK_COMPLIANCE_DETAILS_PAGE_SIZE + 1 }),
        ),
        ("a limit below the modelled minimum", json!({ "Limit": -1 })),
    ] {
        let (status, code) = emulator
            .refused(
                "GetConformancePackComplianceDetails",
                &request("mine", &extras),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the boundary's 400 ValidationException, got {status} {code}"
        );
    }

    // A refusal changes nothing: the pack's recorded results are still reported as they
    // were.
    let (reported, _) = walk("after the refusals", &emulator, "mine", &json!({})).await;
    assert_eq!(
        reported.len(),
        2,
        "a refused request must leave the pack's recorded results as they were, got \
         {reported:#?}"
    );
}

#[tokio::test]
async fn the_report_pages_over_the_rules_and_their_results() {
    // One pack of three rules, each carrying three recorded results, read at a TABLE of
    // page sizes: the rows differ along the size asked for — one result per page, two
    // (which does not divide the total), three (exactly one rule's worth), as many as the
    // pack holds, more than it holds, and the modelled default of 0 (the documented
    // default page).
    let pack = "paged-details-pack";
    let emulator = Emulator::new();
    let rules = deploy(
        "the shared pack",
        &emulator,
        pack,
        &["alpha", "bravo", "charlie"],
    )
    .await;
    let seeded: Vec<(String, Vec<EvaluationResultEntry>)> = rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            let offset = index as u64 * 3;
            (
                rule.clone(),
                vec![
                    result(S3_BUCKET, "logs", DETECTIVE, COMPLIANT, offset),
                    result(DYNAMODB_TABLE, "orders", DETECTIVE, NON_COMPLIANT, offset + 1),
                    result(EC2_VOLUME, "vol-0ab", PROACTIVE, COMPLIANT, offset + 2),
                ],
            )
        })
        .collect();
    let recorded = seed_rules(&emulator, &seeded);
    let all = expected(&recorded, |_| true);

    let cases: [(&str, i64, usize); 6] = [
        ("one result per page", 1, 1),
        ("two results per page", 2, 2),
        ("exactly one rule's worth per page", 3, 3),
        ("as many as the pack holds", 9, 9),
        ("more than the pack holds", 100, 100),
        (
            "the modelled default",
            0,
            GET_CONFORMANCE_PACK_COMPLIANCE_DETAILS_PAGE_SIZE,
        ),
    ];

    for (case, limit, effective) in cases {
        let (reported, sizes) = walk(case, &emulator, pack, &json!({ "Limit": limit })).await;
        assert_eq!(
            reported, all,
            "case {case:?}: paging reports every recorded result once, the pack's rules in \
             name order, got {reported:#?}"
        );
        assert_eq!(
            sizes,
            page_sizes(all.len(), effective),
            "case {case:?}: expected pages of at most {effective} results, got {sizes:?}"
        );
    }

    // A filter and a page size together: the filter narrows what is paged rather than
    // what a page carries, so the pages still cover exactly the selected entries.
    let selected = expected(&recorded, |(_, result)| {
        result.compliance_type == NON_COMPLIANT
    });
    let (reported, sizes) = walk(
        "a filter at one result per page",
        &emulator,
        pack,
        &json!({ "Limit": 1, "Filters": { "ComplianceType": NON_COMPLIANT } }),
    )
    .await;
    assert_eq!(
        reported, selected,
        "a filtered page walk reports exactly the selected entries, got {reported:#?}"
    );
    assert_eq!(
        sizes,
        page_sizes(selected.len(), 1),
        "a filtered page walk serves one selected entry per page, got {sizes:?}"
    );
}
