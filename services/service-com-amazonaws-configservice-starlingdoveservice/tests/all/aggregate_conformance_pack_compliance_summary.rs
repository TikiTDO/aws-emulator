// SPDX-License-Identifier: CC0-1.0

//! `GetAggregateConformancePackComplianceSummary`: how many conformance packs the
//! sources of a configuration aggregator report compliant and noncompliant.
//!
//! The rule pinned here: the operation reports, for every source group the named
//! configuration aggregator takes in — a source account or a source Region, whichever
//! `GroupByKey` names, the account when the request names no key — how many of that
//! group's conformance packs roll up to `COMPLIANT` and how many roll up to
//! `NON_COMPLIANT`, where a pack's own roll-up is the roll-up of the Config rules it
//! deployed (noncompliant if any of them is, compliant if at least one is and none is
//! not, and neither count when no rule places it either way), and echoes the key it
//! grouped by. A source the aggregator does not aggregate, a source the request's
//! `Filters` exclude and a source holding no conformance pack contribute no group at
//! all, and the request is refused for an aggregator that does not exist, for a
//! `GroupByKey` no published value names, for a `NextToken` this operation did not issue
//! and for a `Limit` outside the published range.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines and the deployment helpers
//! `conformance_pack_compliance` defines — over a TABLE of inputs rather than one
//! recorded call: the rows differ along how many packs of which compliance the account
//! holds (including more packs than the twenty-five a `ComplianceSummary` counts rules
//! up to), how many rules each pack deployed and which verdicts they carry, which key
//! the request groups by, which Region it is signed for, which sources the aggregator
//! aggregates, which account and Region its filters name, and which `Limit` it asks for.
//!
//! Recording an evaluation result for a MANAGED rule — which is what a conformance pack
//! template declares — is something Config does itself from the configuration state it
//! recorded, which this emulator does not evaluate, so the rows that need results
//! already recorded seed them into the same shared state a handler reads, through the
//! seed `config_compliance_by_config_rule` already defines. Everything else goes over
//! the wire.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    conformance_pack_rule_name, AGGREGATE_CONFORMANCE_PACK_COMPLIANCE_SUMMARY_PAGE_SIZE,
};

use crate::aggregate_config_rule_compliance_summary::{
    account, account_source, organization_source, put_aggregator, put_whole_account_aggregator,
    AGGREGATOR, HOME_REGION, OTHER_ACCOUNT, OTHER_REGION,
};
use crate::config_compliance_by_config_rule::record_results;
use crate::config_setup_and_rule_loop::Emulator;
use crate::conformance_pack_compliance::template;
use crate::conformance_packs::past_the_deployment;

/// Every published `ComplianceType` value a seeded evaluation result carries here, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`, `NON_COMPLIANT`,
/// `NOT_APPLICABLE`, `INSUFFICIENT_DATA`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// Every published `AggregateConformancePackComplianceSummaryGroupKey` value, from
/// `smithy_query(query="shape",
/// shape="AggregateConformancePackComplianceSummaryGroupKey")`: `ACCOUNT_ID` and
/// `AWS_REGION`.
const ACCOUNT_ID: &str = "ACCOUNT_ID";
/// See [`ACCOUNT_ID`].
const AWS_REGION: &str = "AWS_REGION";

/// The bounds this operation's `Limit` publishes (`Limit`, `@range(min: 0, max: 100)`).
const MIN_LIMIT: i64 = 0;
/// See [`MIN_LIMIT`].
const MAX_LIMIT: i64 = AGGREGATE_CONFORMANCE_PACK_COMPLIANCE_SUMMARY_PAGE_SIZE as i64;

/// The cap a `ComplianceSummary` counts Config RULES up to
/// (`ComplianceSummary$CompliantResourceCount`: "up to a maximum of 25 for rules").
/// `AggregateConformancePackComplianceCount` has no companion `CapExceeded` member, so
/// a count of PACKS is not capped at it — the row holding one pack more than this is
/// what pins that.
const RULE_CAP: usize = 25;

/// One conformance pack a row seeds: the pack's name, and one entry per Config rule its
/// template declares, carrying the evaluation results recorded for that rule.
#[derive(Debug, Clone)]
struct PackSpec {
    name: String,
    rules: Vec<Vec<&'static str>>,
}

/// A pack named `name` whose template declares one Config rule per entry of `rules`,
/// each carrying the evaluation results that entry names.
fn pack(name: &str, rules: Vec<Vec<&'static str>>) -> PackSpec {
    PackSpec {
        name: name.to_string(),
        rules,
    }
}

/// Deploy one conformance pack per entry of `specs`, wait for the deployments the clock
/// owes, and record for every rule a pack deployed the verdicts its entry names.
async fn deploy_packs(case: &str, emulator: &Emulator, specs: &[PackSpec]) {
    let mut declared_per_pack: Vec<Vec<String>> = Vec::new();
    for (index, spec) in specs.iter().enumerate() {
        let declared: Vec<String> = (0..spec.rules.len())
            .map(|ordinal| format!("rule-{index}-{ordinal}"))
            .collect();
        let names: Vec<&str> = declared.iter().map(String::as_str).collect();
        emulator
            .ok(
                "PutConformancePack",
                &json!({
                    "ConformancePackName": spec.name,
                    "TemplateBody": template(&names),
                }),
            )
            .await;
        declared_per_pack.push(declared);
    }

    // A deployment is not finished when `PutConformancePack` answers, so the packs are
    // read back only once the deployment the clock owes is due.
    tokio::time::sleep(past_the_deployment()).await;

    for (spec, declared) in specs.iter().zip(&declared_per_pack) {
        let status = emulator
            .ok(
                "DescribeConformancePackStatus",
                &json!({ "ConformancePackNames": [spec.name] }),
            )
            .await;
        let id = status["ConformancePackStatusDetails"][0]["ConformancePackId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the id of the pack {:?}, got {status}",
                    spec.name
                )
            })
            .to_string();
        for (declared_name, results) in declared.iter().zip(&spec.rules) {
            record_results(
                emulator,
                &conformance_pack_rule_name(declared_name, &id),
                results,
            );
        }
    }
}

/// The `GetAggregateConformancePackComplianceSummary` body for the aggregator every row
/// uses.
fn summary_request() -> Value {
    json!({ "ConfigurationAggregatorName": AGGREGATOR })
}

/// One reported group: the name it carries and the two pack counts its compliance
/// summary reports.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    name: String,
    compliant: i64,
    non_compliant: i64,
}

/// Every group a response reports, in the order it carried them.
fn reported(case: &str, body: &Value) -> Vec<Group> {
    body["AggregateConformancePackComplianceSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected AggregateConformancePackComplianceSummaries, got {body}"
            )
        })
        .iter()
        .map(|entry| {
            let name = entry["GroupName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a GroupName, got {entry}"))
                .to_string();
            let summary = &entry["ComplianceSummary"];
            assert!(
                summary.is_object(),
                "case {case:?}: expected a ComplianceSummary, got {entry}"
            );
            Group {
                name,
                // A count the response leaves out is the modelled default of 0.
                compliant: summary
                    .get("CompliantConformancePackCount")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
                non_compliant: summary
                    .get("NonCompliantConformancePackCount")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// The one group a response is expected to carry, named `name`.
fn only_group(case: &str, body: &Value, name: &str) -> Group {
    let groups = reported(case, body);
    assert_eq!(
        groups.len(),
        1,
        "case {case:?}: expected exactly one group, got {groups:?}"
    );
    let group = groups.into_iter().next().expect("the group was counted");
    assert_eq!(
        group.name, name,
        "case {case:?}: expected the group {name:?}, got {group:?}"
    );
    group
}

/// How many of `specs`' packs the pack's OWN compliance report places compliant and how
/// many noncompliant, read back over the wire through
/// `DescribeConformancePackCompliance` — the same roll-up, asked of the other reader.
async fn counted_per_pack(case: &str, emulator: &Emulator, specs: &[PackSpec]) -> (i64, i64) {
    let mut compliant = 0i64;
    let mut non_compliant = 0i64;
    for spec in specs {
        let body = emulator
            .ok(
                "DescribeConformancePackCompliance",
                &json!({ "ConformancePackName": spec.name }),
            )
            .await;
        let types: Vec<String> = body["ConformancePackRuleComplianceList"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the pack's rule compliance, got {body}")
            })
            .iter()
            .filter_map(|entry| entry["ComplianceType"].as_str().map(str::to_string))
            .collect();
        if types.iter().any(|placed| placed == NON_COMPLIANT) {
            non_compliant += 1;
        } else if types.iter().any(|placed| placed == COMPLIANT) {
            compliant += 1;
        }
    }
    (compliant, non_compliant)
}

#[tokio::test]
async fn counts_the_compliant_and_noncompliant_packs_of_the_aggregated_account() {
    struct Case {
        case: &'static str,
        specs: Vec<PackSpec>,
        compliant: i64,
        non_compliant: i64,
    }

    // One pack per name, each declaring one Config rule per verdict list. Rows differ
    // along what the packs' rules were placed at, how many rules a pack has, and how
    // many packs the account holds.
    let cases = vec![
        Case {
            case: "one pack whose only rule is compliant",
            specs: vec![pack("alpha", vec![vec![COMPLIANT]])],
            compliant: 1,
            non_compliant: 0,
        },
        Case {
            case: "one pack whose only rule carries no result at all",
            specs: vec![pack("alpha", vec![vec![]])],
            compliant: 0,
            non_compliant: 0,
        },
        Case {
            case: "one pack whose only rule is noncompliant",
            specs: vec![pack("alpha", vec![vec![NON_COMPLIANT]])],
            compliant: 0,
            non_compliant: 1,
        },
        Case {
            case: "a pack is noncompliant when any of its rules is",
            specs: vec![pack(
                "alpha",
                vec![vec![COMPLIANT], vec![NON_COMPLIANT], vec![COMPLIANT]],
            )],
            compliant: 0,
            non_compliant: 1,
        },
        Case {
            case: "a pack whose placed rules are all compliant is compliant",
            specs: vec![pack("alpha", vec![vec![COMPLIANT], vec![]])],
            compliant: 1,
            non_compliant: 0,
        },
        Case {
            case: "only results that place no rule either way",
            specs: vec![pack("alpha", vec![vec![NOT_APPLICABLE, NOT_APPLICABLE]])],
            compliant: 0,
            non_compliant: 0,
        },
        Case {
            case: "a pack at each of the three verdicts",
            specs: vec![
                pack("alpha", vec![vec![COMPLIANT, COMPLIANT]]),
                pack("bravo", vec![vec![NON_COMPLIANT]]),
                pack("charlie", vec![vec![]]),
            ],
            compliant: 1,
            non_compliant: 1,
        },
        Case {
            case: "several packs on each count",
            specs: vec![
                pack("alpha", vec![vec![COMPLIANT]]),
                pack("bravo", vec![vec![COMPLIANT], vec![COMPLIANT]]),
                pack("charlie", vec![vec![NON_COMPLIANT]]),
                pack("delta", vec![vec![COMPLIANT], vec![NON_COMPLIANT]]),
                pack("echo", vec![vec![NON_COMPLIANT, COMPLIANT]]),
            ],
            compliant: 2,
            non_compliant: 3,
        },
        Case {
            case: "one pack more than the twenty-five a rule summary counts up to",
            specs: (0..=RULE_CAP)
                .map(|ordinal| pack(&format!("pack-{ordinal}"), vec![vec![COMPLIANT]]))
                .collect(),
            compliant: RULE_CAP as i64 + 1,
            non_compliant: 0,
        },
    ];

    for Case {
        case,
        specs,
        compliant,
        non_compliant,
    } in cases
    {
        let emulator = Emulator::in_region(HOME_REGION);
        deploy_packs(case, &emulator, &specs).await;
        put_whole_account_aggregator(&emulator).await;

        let body = emulator
            .ok(
                "GetAggregateConformancePackComplianceSummary",
                &summary_request(),
            )
            .await;
        let group = only_group(case, &body, &account());
        assert_eq!(
            (group.compliant, group.non_compliant),
            (compliant, non_compliant),
            "case {case:?}: expected {compliant} compliant and {non_compliant} noncompliant \
             packs, got {group:?}"
        );
        assert!(
            body.get("NextToken").is_none_or(Value::is_null),
            "case {case:?}: expected no NextToken for a single group, got {body}"
        );

        // The same packs, counted from each pack's OWN compliance report: an aggregate
        // count of this account's packs and the packs' own reports cannot disagree.
        let per_pack = counted_per_pack(case, &emulator, &specs).await;
        assert_eq!(
            per_pack,
            (compliant, non_compliant),
            "case {case:?}: expected the packs' own compliance reports to carry the same \
             counts, got {per_pack:?}"
        );

        // The read is a report: it must not change what the next reader sees.
        let again = emulator
            .ok(
                "GetAggregateConformancePackComplianceSummary",
                &summary_request(),
            )
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read must report the same counts, got {again}"
        );
    }
}

#[tokio::test]
async fn names_each_group_by_the_key_the_request_groups_by() {
    struct Case {
        case: &'static str,
        region: &'static str,
        group_by_key: Option<&'static str>,
        /// The group name expected, or `None` for this emulator's own account id.
        group_name: Option<&'static str>,
        echoed: &'static str,
    }

    let cases = [
        Case {
            case: "no key names the source account",
            region: HOME_REGION,
            group_by_key: None,
            group_name: None,
            echoed: ACCOUNT_ID,
        },
        Case {
            case: "grouped by account",
            region: HOME_REGION,
            group_by_key: Some(ACCOUNT_ID),
            group_name: None,
            echoed: ACCOUNT_ID,
        },
        Case {
            case: "grouped by Region",
            region: HOME_REGION,
            group_by_key: Some(AWS_REGION),
            group_name: Some(HOME_REGION),
            echoed: AWS_REGION,
        },
        Case {
            case: "grouped by Region, signed for another Region",
            region: OTHER_REGION,
            group_by_key: Some(AWS_REGION),
            group_name: Some(OTHER_REGION),
            echoed: AWS_REGION,
        },
        Case {
            case: "grouped by account, signed for another Region",
            region: OTHER_REGION,
            group_by_key: Some(ACCOUNT_ID),
            group_name: None,
            echoed: ACCOUNT_ID,
        },
    ];

    for Case {
        case,
        region,
        group_by_key,
        group_name,
        echoed,
    } in cases
    {
        let emulator = Emulator::in_region(region);
        deploy_packs(
            case,
            &emulator,
            &[pack("alpha", vec![vec![NON_COMPLIANT]])],
        )
        .await;
        put_whole_account_aggregator(&emulator).await;

        let mut request = summary_request();
        if let Some(key) = group_by_key {
            request["GroupByKey"] = json!(key);
        }
        let body = emulator
            .ok("GetAggregateConformancePackComplianceSummary", &request)
            .await;

        let expected = group_name.map_or_else(account, str::to_string);
        let group = only_group(case, &body, &expected);
        assert_eq!(
            (group.compliant, group.non_compliant),
            (0, 1),
            "case {case:?}: expected the group's noncompliant pack count, got {group:?}"
        );
        assert_eq!(
            body["GroupByKey"].as_str(),
            Some(echoed),
            "case {case:?}: expected the GroupByKey {echoed:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn reports_only_the_sources_the_aggregator_and_the_filters_admit() {
    let account = account();
    let whole_account = account_source(&[&account], &[]);
    // (case, the aggregator's sources, the request's Filters, whether a group is
    // expected)
    let cases: Vec<(&str, Value, Option<Value>, bool)> = vec![
        (
            "this account in every Region",
            whole_account.clone(),
            None,
            true,
        ),
        (
            "this account in this Region",
            account_source(&[&account], &[HOME_REGION]),
            None,
            true,
        ),
        (
            "this account in another Region only",
            account_source(&[&account], &[OTHER_REGION]),
            None,
            false,
        ),
        (
            "another account in every Region",
            account_source(&[OTHER_ACCOUNT], &[]),
            None,
            false,
        ),
        (
            "this account among several sources",
            account_source(&[OTHER_ACCOUNT, &account], &[HOME_REGION, OTHER_REGION]),
            None,
            true,
        ),
        (
            "an organization over every Region",
            organization_source(&[]),
            None,
            true,
        ),
        (
            "an organization over another Region",
            organization_source(&[OTHER_REGION]),
            None,
            false,
        ),
        (
            "a filter naming this account",
            whole_account.clone(),
            Some(json!({ "AccountId": account })),
            true,
        ),
        (
            "a filter naming another account",
            whole_account.clone(),
            Some(json!({ "AccountId": OTHER_ACCOUNT })),
            false,
        ),
        (
            "a filter naming this Region",
            whole_account.clone(),
            Some(json!({ "AwsRegion": HOME_REGION })),
            true,
        ),
        (
            "a filter naming another Region",
            whole_account.clone(),
            Some(json!({ "AwsRegion": OTHER_REGION })),
            false,
        ),
        (
            "filters naming this account and this Region",
            whole_account.clone(),
            Some(json!({ "AccountId": account, "AwsRegion": HOME_REGION })),
            true,
        ),
        (
            "filters naming this account in another Region",
            whole_account.clone(),
            Some(json!({ "AccountId": account, "AwsRegion": OTHER_REGION })),
            false,
        ),
    ];

    for (case, sources, filters, expected) in cases {
        let emulator = Emulator::in_region(HOME_REGION);
        deploy_packs(case, &emulator, &[pack("alpha", vec![vec![COMPLIANT]])]).await;
        put_aggregator(&emulator, sources).await;

        let mut request = summary_request();
        if let Some(filters) = filters {
            request["Filters"] = filters;
        }
        let body = emulator
            .ok("GetAggregateConformancePackComplianceSummary", &request)
            .await;
        let groups = reported(case, &body);

        if expected {
            let group = only_group(case, &body, &account);
            assert_eq!(
                (group.compliant, group.non_compliant),
                (1, 0),
                "case {case:?}: expected the admitted group's compliant pack count, got {group:?}"
            );
        } else {
            assert!(
                groups.is_empty(),
                "case {case:?}: expected no group, got {groups:?}"
            );
        }
    }
}

#[tokio::test]
async fn reports_a_zero_valued_group_for_an_aggregated_account_with_no_conformance_packs() {
    let emulator = Emulator::in_region(HOME_REGION);
    put_whole_account_aggregator(&emulator).await;

    let body = emulator
        .ok(
            "GetAggregateConformancePackComplianceSummary",
            &summary_request(),
        )
        .await;
    // A group the aggregator takes in is reported whatever it holds: no pack is zero
    // compliant and zero noncompliant packs, not an absent row.
    let group = only_group("no packs to summarise", &body, &account());
    assert_eq!(
        (group.compliant, group.non_compliant),
        (0, 0),
        "no packs to summarise: expected a zero-valued summary row, got {group:?}"
    );
    assert_eq!(
        body["GroupByKey"].as_str(),
        Some(ACCOUNT_ID),
        "no packs to summarise: expected the GroupByKey to be reported, got {body}"
    );
}

#[tokio::test]
async fn serves_every_published_limit_and_refuses_one_outside_the_range() {
    // (case, the Limit asked for, whether it is served)
    let cases: [(&str, i64, bool); 5] = [
        ("the published minimum asks for the default", MIN_LIMIT, true),
        ("one group per page", 1, true),
        ("the published maximum", MAX_LIMIT, true),
        ("one past the maximum", MAX_LIMIT + 1, false),
        ("one past the minimum", MIN_LIMIT - 1, false),
    ];

    let emulator = Emulator::in_region(HOME_REGION);
    deploy_packs(
        "the shared pack",
        &emulator,
        &[pack("alpha", vec![vec![COMPLIANT]])],
    )
    .await;
    put_whole_account_aggregator(&emulator).await;

    for (case, limit, served) in cases {
        let mut request = summary_request();
        request["Limit"] = json!(limit);
        if served {
            let body = emulator
                .ok("GetAggregateConformancePackComplianceSummary", &request)
                .await;
            let group = only_group(case, &body, &account());
            assert_eq!(
                (group.compliant, group.non_compliant),
                (1, 0),
                "case {case:?}: expected the group's compliant pack count, got {group:?}"
            );
            assert!(
                body.get("NextToken").is_none_or(Value::is_null),
                "case {case:?}: expected no NextToken once every group is served, got {body}"
            );
        } else {
            // The shared request validation refuses the modelled range before the
            // handler's own `InvalidLimitException` can answer for it, so either code
            // is a correct refusal of a limit outside the allowable range.
            let (status, code) = emulator
                .refused("GetAggregateConformancePackComplianceSummary", &request)
                .await;
            assert!(
                status == 400
                    && ["InvalidLimitException", "ValidationException"].contains(&code.as_str()),
                "case {case:?}: expected HTTP 400 for a limit outside the range, got \
                 {status} {code}"
            );
        }
    }
}

#[tokio::test]
async fn refuses_an_unknown_aggregator_an_unknown_group_key_and_a_token_it_did_not_issue() {
    let emulator = Emulator::in_region(HOME_REGION);
    deploy_packs(
        "the shared pack",
        &emulator,
        &[pack("alpha", vec![vec![COMPLIANT]])],
    )
    .await;
    put_whole_account_aggregator(&emulator).await;

    // (case, the request, the expected error code)
    let cases: [(&str, Value, &str); 5] = [
        (
            "an aggregator that does not exist",
            json!({ "ConfigurationAggregatorName": "no-such-aggregator" }),
            "NoSuchConfigurationAggregatorException",
        ),
        (
            "an aggregator that does not exist, grouped by Region",
            json!({
                "ConfigurationAggregatorName": "no-such-aggregator",
                "GroupByKey": AWS_REGION,
            }),
            "NoSuchConfigurationAggregatorException",
        ),
        (
            "a token that is not base64 at all",
            json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "NextToken": "**not base64**",
            }),
            "InvalidNextTokenException",
        ),
        (
            "a token outside the token codec's own base64 form",
            json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "NextToken": "bm90LWEtZ3JvdXA",
            }),
            "InvalidNextTokenException",
        ),
        (
            "a group key no published value names",
            json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "GroupByKey": "RESOURCE_TYPE",
            }),
            "ValidationException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, code) = emulator
            .refused("GetAggregateConformancePackComplianceSummary", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected HTTP 400 {expected}, got {status} {code}"
        );
    }

    // A refusal changes nothing: the account's packs are still counted as they were.
    let body = emulator
        .ok(
            "GetAggregateConformancePackComplianceSummary",
            &summary_request(),
        )
        .await;
    let group = only_group("after the refusals", &body, &account());
    assert_eq!(
        (group.compliant, group.non_compliant),
        (1, 0),
        "after the refusals: expected the counts to stand, got {group:?}"
    );
}
