// SPDX-License-Identifier: CC0-1.0

//! `DescribeAggregateComplianceByConformancePacks`: the compliance of every conformance
//! pack the sources of a configuration aggregator hold.
//!
//! The rule pinned here: the operation reports one entry per conformance pack a source
//! the named aggregator takes in holds — the pack's name, the source account and Region
//! its data is aggregated from, the `ComplianceType` the Config rules THAT PACK deployed
//! roll up to (`NON_COMPLIANT` if any of them is, `COMPLIANT` if at least one is and none
//! is not, `INSUFFICIENT_DATA` when no rule places it either way), and how many of those
//! rules are compliant, how many are noncompliant and how many there are in total, the
//! total counting every rule the pack deployed including the ones no result places either
//! way. The report is narrowed, never changed, by the `Filters` the request carries — one
//! source account, one source Region, one pack name, one compliance status — paged over
//! the packs' name ordering, and refuses an aggregator that does not exist, a compliance
//! type no published value names, a token the operation did not issue and a `Limit`
//! outside the published range.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines, the deployment helper
//! `conformance_pack_compliance` defines and the aggregator helpers
//! `aggregate_config_rule_compliance_summary` defines — over a TABLE of inputs rather
//! than one recorded call: the rows differ along how many packs the account holds, how
//! many rules each pack deployed and which verdicts they carry, which Region the request
//! is signed for, which sources the aggregator aggregates, which account, Region, pack
//! name and compliance status its filters name, and which `Limit` it asks for.
//!
//! Recording an evaluation result for a MANAGED rule — which is what a conformance pack
//! template declares — is something Config does itself from the configuration state it
//! recorded, which this emulator does not evaluate, so the rows that need results already
//! recorded seed them into the same shared state a handler reads, through the seed
//! `config_compliance_by_config_rule` already defines. Everything else goes over the
//! wire.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::AGGREGATE_COMPLIANCE_BY_CONFORMANCE_PACKS_PAGE_SIZE;

use crate::aggregate_config_rule_compliance_summary::{
    account, account_source, organization_source, put_aggregator, put_whole_account_aggregator,
    AGGREGATOR, HOME_REGION, OTHER_ACCOUNT, OTHER_REGION,
};
use crate::config_compliance_by_config_rule::record_results;
use crate::config_setup_and_rule_loop::Emulator;
use crate::conformance_pack_compliance::deploy;

/// Every published `ConformancePackComplianceType` value, from
/// `smithy_query(query="shape", shape="ConformancePackComplianceType")`: `COMPLIANT`,
/// `NON_COMPLIANT`, `INSUFFICIENT_DATA`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// A `ComplianceType` an evaluation result can carry that places a rule at no conformance
/// pack compliance type (`com.amazonaws.configservice#ComplianceType` has it, so a result
/// can be recorded with it; `ConformancePackComplianceType` does not).
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// The bounds this operation's `Limit` publishes (`Limit`, `@range(min: 0, max: 100)`).
const MIN_LIMIT: i64 = 0;
/// See [`MIN_LIMIT`].
const MAX_LIMIT: i64 = AGGREGATE_COMPLIANCE_BY_CONFORMANCE_PACKS_PAGE_SIZE as i64;

/// One conformance pack a row seeds: the pack's name, and one entry per Config rule its
/// template declares, carrying the evaluation results recorded for that rule.
#[derive(Debug, Clone)]
struct PackSpec {
    name: String,
    rules: Vec<Vec<&'static str>>,
}

/// A pack named `name` whose template declares one Config rule per entry of `rules`, each
/// carrying the evaluation results that entry names.
fn pack(name: &str, rules: Vec<Vec<&'static str>>) -> PackSpec {
    PackSpec {
        name: name.to_string(),
        rules,
    }
}

/// Deploy one conformance pack per entry of `specs` and record for every rule a pack
/// deployed the verdicts its entry names.
async fn deploy_packs(case: &str, emulator: &Emulator, specs: &[PackSpec]) {
    for (index, spec) in specs.iter().enumerate() {
        let declared: Vec<String> = (0..spec.rules.len())
            .map(|ordinal| format!("rule-{index}-{ordinal}"))
            .collect();
        let names: Vec<&str> = declared.iter().map(String::as_str).collect();
        let deployed = deploy(case, emulator, &spec.name, &names).await;
        for (name, results) in deployed.iter().zip(&spec.rules) {
            record_results(emulator, name, results);
        }
    }
}

/// The `DescribeAggregateComplianceByConformancePacks` body for the aggregator every row
/// uses.
fn read_request() -> Value {
    json!({ "ConfigurationAggregatorName": AGGREGATOR })
}

/// One reported pack: its name, the compliance status it carries, its three rule counts
/// and the source it is aggregated from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    name: String,
    compliance_type: String,
    compliant: i64,
    non_compliant: i64,
    total: i64,
    account_id: String,
    aws_region: String,
}

/// Every pack a response reports, in the order it carried them.
fn reported(case: &str, body: &Value) -> Vec<Reported> {
    body["AggregateComplianceByConformancePacks"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected AggregateComplianceByConformancePacks, got {body}")
        })
        .iter()
        .map(|entry| {
            let name = entry["ConformancePackName"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a ConformancePackName, got {entry}")
                })
                .to_string();
            let compliance = &entry["Compliance"];
            let compliance_type = compliance["ComplianceType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ComplianceType, got {entry}"))
                .to_string();
            // A count the response leaves out is the modelled default of 0.
            let count = |member: &str| {
                compliance
                    .get(member)
                    .and_then(Value::as_i64)
                    .unwrap_or_default()
            };
            Reported {
                name,
                compliance_type,
                compliant: count("CompliantRuleCount"),
                non_compliant: count("NonCompliantRuleCount"),
                total: count("TotalRuleCount"),
                account_id: entry["AccountId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an AccountId, got {entry}"))
                    .to_string(),
                aws_region: entry["AwsRegion"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an AwsRegion, got {entry}"))
                    .to_string(),
            }
        })
        .collect()
}

/// The names a response reports, in the order it reported them.
fn reported_names(case: &str, body: &Value) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

/// How the pack `name`'s OWN compliance report places its rules, read back over the wire
/// through `DescribeConformancePackCompliance` — the same per-rule roll-up, asked of the
/// other reader.
async fn per_rule_compliance(case: &str, emulator: &Emulator, name: &str) -> Vec<String> {
    let body = emulator
        .ok(
            "DescribeConformancePackCompliance",
            &json!({ "ConformancePackName": name }),
        )
        .await;
    body["ConformancePackRuleComplianceList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected the pack's rule compliance, got {body}"))
        .iter()
        .map(|entry| {
            entry["ComplianceType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ComplianceType, got {entry}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn reports_each_packs_compliance_status_and_rule_counts() {
    struct Case {
        case: &'static str,
        specs: Vec<PackSpec>,
        /// One `(ComplianceType, CompliantRuleCount, NonCompliantRuleCount,
        /// TotalRuleCount)` per pack, in the packs' name ordering.
        expected: Vec<(&'static str, i64, i64, i64)>,
    }

    // Rows differ along what the packs' rules were placed at, how many rules a pack
    // declared and how many packs the account holds.
    let cases = vec![
        Case {
            case: "one pack whose only rule is compliant",
            specs: vec![pack("alpha", vec![vec![COMPLIANT]])],
            expected: vec![(COMPLIANT, 1, 0, 1)],
        },
        Case {
            case: "one pack whose only rule is noncompliant",
            specs: vec![pack("alpha", vec![vec![NON_COMPLIANT]])],
            expected: vec![(NON_COMPLIANT, 0, 1, 1)],
        },
        Case {
            case: "one pack whose only rule carries no result at all",
            specs: vec![pack("alpha", vec![vec![]])],
            expected: vec![(INSUFFICIENT_DATA, 0, 0, 1)],
        },
        Case {
            case: "a pack whose rules are placed at neither status",
            specs: vec![pack(
                "alpha",
                vec![vec![NOT_APPLICABLE, NOT_APPLICABLE], vec![NOT_APPLICABLE]],
            )],
            expected: vec![(INSUFFICIENT_DATA, 0, 0, 2)],
        },
        Case {
            case: "a pack is noncompliant when any of its rules is",
            specs: vec![pack(
                "alpha",
                vec![vec![COMPLIANT], vec![NON_COMPLIANT], vec![COMPLIANT]],
            )],
            expected: vec![(NON_COMPLIANT, 2, 1, 3)],
        },
        Case {
            case: "an unevaluated rule counts in the total only",
            specs: vec![pack(
                "alpha",
                vec![vec![COMPLIANT], vec![], vec![NOT_APPLICABLE]],
            )],
            expected: vec![(COMPLIANT, 1, 0, 3)],
        },
        Case {
            case: "several verdicts for one rule roll up to one count",
            specs: vec![pack(
                "alpha",
                vec![
                    vec![COMPLIANT, COMPLIANT, COMPLIANT],
                    vec![NON_COMPLIANT, COMPLIANT],
                ],
            )],
            expected: vec![(NON_COMPLIANT, 1, 1, 2)],
        },
        Case {
            case: "a pack at each of the three statuses, reported in name ordering",
            specs: vec![
                pack("charlie", vec![vec![]]),
                pack("alpha", vec![vec![COMPLIANT], vec![COMPLIANT]]),
                pack("bravo", vec![vec![NON_COMPLIANT], vec![COMPLIANT]]),
            ],
            expected: vec![
                (COMPLIANT, 2, 0, 2),
                (NON_COMPLIANT, 1, 1, 2),
                (INSUFFICIENT_DATA, 0, 0, 1),
            ],
        },
    ];

    for Case {
        case,
        specs,
        expected,
    } in cases
    {
        let emulator = Emulator::in_region(HOME_REGION);
        deploy_packs(case, &emulator, &specs).await;
        put_whole_account_aggregator(&emulator).await;

        let body = emulator
            .ok("DescribeAggregateComplianceByConformancePacks", &read_request())
            .await;
        let packs = reported(case, &body);

        let mut sorted: Vec<String> = specs.iter().map(|spec| spec.name.clone()).collect();
        sorted.sort();
        let actual: Vec<(String, String, i64, i64, i64)> = packs
            .iter()
            .map(|entry| {
                (
                    entry.name.clone(),
                    entry.compliance_type.clone(),
                    entry.compliant,
                    entry.non_compliant,
                    entry.total,
                )
            })
            .collect();
        let wanted: Vec<(String, String, i64, i64, i64)> = sorted
            .iter()
            .zip(&expected)
            .map(|(name, (status, compliant, non_compliant, total))| {
                (
                    name.clone(),
                    (*status).to_string(),
                    *compliant,
                    *non_compliant,
                    *total,
                )
            })
            .collect();
        assert_eq!(
            actual, wanted,
            "case {case:?}: expected {wanted:?}, got {actual:?}"
        );

        for entry in &packs {
            assert_eq!(
                (entry.account_id.as_str(), entry.aws_region.as_str()),
                (account().as_str(), HOME_REGION),
                "case {case:?}: expected the source account and Region, got {entry:?}"
            );
            // The pack's OWN compliance report must place its rules the same way: an
            // aggregate read and the pack's own reader cannot disagree about what Config
            // found. They differ in one thing only, and the rows above name it: the
            // aggregate TOTAL counts every rule the pack deployed, while the pack's own
            // report carries a rule only once a result places it, so it is a subset.
            let placed = per_rule_compliance(case, &emulator, &entry.name).await;
            let compliant = placed.iter().filter(|at| *at == COMPLIANT).count() as i64;
            let non_compliant = placed.iter().filter(|at| *at == NON_COMPLIANT).count() as i64;
            assert_eq!(
                (compliant, non_compliant),
                (entry.compliant, entry.non_compliant),
                "case {case:?}: expected the pack's own report to agree, got {placed:?} for \
                 {entry:?}"
            );
            assert!(
                (placed.len() as i64) <= entry.total,
                "case {case:?}: the pack reports at most one rule per rule it deployed, got \
                 {placed:?} for {entry:?}"
            );
        }
        assert!(
            body.get("NextToken").is_none_or(Value::is_null),
            "case {case:?}: expected no NextToken once every pack is served, got {body}"
        );

        // The read is a report: it must not change what the next reader sees.
        let again = emulator
            .ok("DescribeAggregateComplianceByConformancePacks", &read_request())
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read must report the same packs, got {again}"
        );
    }
}

#[tokio::test]
async fn reports_only_the_sources_the_aggregator_and_the_filters_admit() {
    let account = account();
    let whole_account = account_source(&[&account], &[]);
    // (case, the aggregator's sources, the request's Filters, whether the pack is
    // reported)
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

        let mut request = read_request();
        if let Some(filters) = filters {
            request["Filters"] = filters;
        }
        let body = emulator
            .ok("DescribeAggregateComplianceByConformancePacks", &request)
            .await;
        let names = reported_names(case, &body);
        let wanted: Vec<String> = if expected {
            vec!["alpha".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(
            names, wanted,
            "case {case:?}: expected {wanted:?}, got {names:?}"
        );
    }
}

#[tokio::test]
async fn reports_an_empty_page_for_an_aggregated_account_holding_no_conformance_pack() {
    let emulator = Emulator::in_region(HOME_REGION);
    put_whole_account_aggregator(&emulator).await;

    let body = emulator
        .ok("DescribeAggregateComplianceByConformancePacks", &read_request())
        .await;
    let names = reported_names("no packs to report", &body);
    assert!(
        names.is_empty(),
        "no packs to report: expected an empty result page, got {names:?}"
    );
    assert!(
        body.get("NextToken").is_none_or(Value::is_null),
        "no packs to report: expected no NextToken, got {body}"
    );
}

#[tokio::test]
async fn selects_by_pack_name_and_by_compliance_status() {
    let specs = vec![
        pack("alpha", vec![vec![COMPLIANT]]),
        pack("bravo", vec![vec![NON_COMPLIANT]]),
        pack("charlie", vec![vec![COMPLIANT], vec![COMPLIANT]]),
        pack("delta", vec![vec![NOT_APPLICABLE]]),
    ];

    // (case, the Filters the request carries, the pack names expected)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filter at all", json!({}), vec![
            "alpha", "bravo", "charlie", "delta",
        ]),
        (
            "one pack by name",
            json!({ "ConformancePackName": "bravo" }),
            vec!["bravo"],
        ),
        (
            "another pack by name",
            json!({ "ConformancePackName": "charlie" }),
            vec!["charlie"],
        ),
        (
            "a name this account holds no pack under",
            json!({ "ConformancePackName": "nosuchpack" }),
            Vec::new(),
        ),
        (
            "the compliant packs",
            json!({ "ComplianceType": COMPLIANT }),
            vec!["alpha", "charlie"],
        ),
        (
            "the noncompliant packs",
            json!({ "ComplianceType": NON_COMPLIANT }),
            vec!["bravo"],
        ),
        (
            "the packs no rule of theirs places either way",
            json!({ "ComplianceType": INSUFFICIENT_DATA }),
            vec!["delta"],
        ),
        (
            "a name and the status that pack carries",
            json!({ "ConformancePackName": "alpha", "ComplianceType": COMPLIANT }),
            vec!["alpha"],
        ),
        (
            "a name and a status that pack does not carry",
            json!({ "ConformancePackName": "alpha", "ComplianceType": NON_COMPLIANT }),
            Vec::new(),
        ),
    ];

    let emulator = Emulator::in_region(HOME_REGION);
    deploy_packs("the shared packs", &emulator, &specs).await;
    put_whole_account_aggregator(&emulator).await;

    for (case, filters, expected) in cases {
        let mut request = read_request();
        request["Filters"] = filters;
        let body = emulator
            .ok("DescribeAggregateComplianceByConformancePacks", &request)
            .await;
        let names = reported_names(case, &body);
        assert_eq!(
            names,
            expected.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected {expected:?}, got {names:?}"
        );
    }
}

#[tokio::test]
async fn pages_over_the_packs_name_ordering() {
    let specs: Vec<PackSpec> = ["alpha", "bravo", "charlie", "delta", "echo"]
        .iter()
        .map(|name| pack(name, vec![vec![COMPLIANT]]))
        .collect();
    let every_pack: Vec<String> = {
        let mut names: Vec<String> = specs.iter().map(|spec| spec.name.clone()).collect();
        names.sort();
        names
    };

    let emulator = Emulator::in_region(HOME_REGION);
    deploy_packs("the shared packs", &emulator, &specs).await;
    put_whole_account_aggregator(&emulator).await;

    // Rows differ along the page size asked for: one pack per page, several per page, a
    // page exactly as large as the set, and one larger than it.
    for page_size in [1usize, 2, 3, every_pack.len(), every_pack.len() + 1] {
        let case = format!("{page_size} packs per page");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = read_request();
            request["Limit"] = json!(page_size);
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let body = emulator
                .ok("DescribeAggregateComplianceByConformancePacks", &request)
                .await;
            let names = reported_names(&case, &body);
            assert!(
                names.len() <= page_size,
                "case {case:?}: expected at most {page_size} packs per page, got {names:?}"
            );
            walked.extend(names);
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= every_pack.len(),
                "case {case:?}: the page walk must terminate, got {walked:?}"
            );
        }
        assert_eq!(
            walked, every_pack,
            "case {case:?}: expected every pack once in name ordering, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn serves_every_published_limit_and_refuses_one_outside_the_range() {
    // (case, the Limit asked for, whether it is served)
    let cases: [(&str, i64, bool); 5] = [
        ("the published minimum asks for the default", MIN_LIMIT, true),
        ("one pack per page", 1, true),
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
        let mut request = read_request();
        request["Limit"] = json!(limit);
        if served {
            let body = emulator
                .ok("DescribeAggregateComplianceByConformancePacks", &request)
                .await;
            let names = reported_names(case, &body);
            assert_eq!(
                names,
                vec!["alpha".to_string()],
                "case {case:?}: expected the account's one pack, got {names:?}"
            );
            assert!(
                body.get("NextToken").is_none_or(Value::is_null),
                "case {case:?}: expected no NextToken once every pack is served, got {body}"
            );
        } else {
            // The shared request validation refuses the modelled range before the
            // handler's own `InvalidLimitException` can answer for it, so either code is
            // a correct refusal of a limit outside the allowable range.
            let (status, code) = emulator
                .refused("DescribeAggregateComplianceByConformancePacks", &request)
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
async fn refuses_an_unknown_aggregator_an_unheld_compliance_type_and_a_token_it_did_not_issue() {
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
            "an aggregator that does not exist, filtered by pack name",
            json!({
                "ConfigurationAggregatorName": "no-such-aggregator",
                "Filters": { "ConformancePackName": "alpha" },
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
                "NextToken": "bm90LWEtcGFjaw",
            }),
            "InvalidNextTokenException",
        ),
        (
            "a compliance type no published value names",
            json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "Filters": { "ComplianceType": NOT_APPLICABLE },
            }),
            "ValidationException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, code) = emulator
            .refused("DescribeAggregateComplianceByConformancePacks", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected HTTP 400 {expected}, got {status} {code}"
        );
    }

    // A refusal changes nothing: the account's pack is still reported as it was.
    let body = emulator
        .ok("DescribeAggregateComplianceByConformancePacks", &read_request())
        .await;
    let names = reported_names("after the refusals", &body);
    assert_eq!(
        names,
        vec!["alpha".to_string()],
        "after the refusals: expected the pack to stand, got {names:?}"
    );
}
