// SPDX-License-Identifier: CC0-1.0

//! `GetAggregateConfigRuleComplianceSummary`: how many Config rules the sources of a
//! configuration aggregator report compliant and noncompliant.
//!
//! The rule pinned here: the operation reports, for every source group the named
//! configuration aggregator takes in — a source account or a source Region, whichever
//! `GroupByKey` names, the account when the request names no key — how many of that
//! group's Config rules have evaluation results rolling up to `COMPLIANT` and how many
//! roll up to `NON_COMPLIANT`, each counted up to the twenty-five-rule cap a
//! `ComplianceSummary` reports contributors up to, and echoes the key it grouped by. A
//! source the aggregator does not aggregate, a source the request's `Filters` exclude
//! and a source holding no Config rules contribute no group at all, and the request is
//! refused for an aggregator that does not exist, for a `GroupByKey` no published value
//! names, for a `NextToken` this operation did not issue and for a `Limit` outside the
//! published range.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds, using the
//! `Emulator` harness `config_setup_and_rule_loop` defines — over a TABLE of inputs
//! rather than one recorded call: the rows differ along how many rules of which
//! compliance the account holds (including the counts at the cap and one past it),
//! which key the request groups by, which Region it is signed for, which sources the
//! aggregator aggregates, which account and Region its filters name, and which `Limit`
//! it asks for.
//!
//! Recording an evaluation result for a MANAGED rule is something Config does itself
//! from the configuration state it recorded, which this emulator does not evaluate, so
//! the rows that need results already recorded seed them into the same shared state a
//! handler reads, through the seed `config_compliance_by_config_rule` already defines.
//! A rule whose verdicts are DELIVERED to Config is driven over the wire in
//! `put_evaluations`, which pins the same summary surface end to end. Everything else
//! goes over the wire.

use serde_json::{json, Value};

use crate::config_compliance_by_config_rule::record_results;
use crate::config_setup_and_rule_loop::{
    managed_rule, with_recorder_and_channel, Emulator, ROLE_ARN,
};

/// Every published `ComplianceType` value, from
/// `smithy_query(query="shape", shape="ComplianceType")`: `COMPLIANT`, `NON_COMPLIANT`,
/// `NOT_APPLICABLE`, `INSUFFICIENT_DATA`.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";
/// See [`COMPLIANT`].
const NOT_APPLICABLE: &str = "NOT_APPLICABLE";
/// See [`COMPLIANT`].
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// Every published `ConfigRuleComplianceSummaryGroupKey` value, from
/// `smithy_query(query="shape", shape="ConfigRuleComplianceSummaryGroupKey")`:
/// `ACCOUNT_ID` and `AWS_REGION`.
const ACCOUNT_ID: &str = "ACCOUNT_ID";
/// See [`ACCOUNT_ID`].
const AWS_REGION: &str = "AWS_REGION";

/// The cap a `ComplianceSummary` counts Config rules up to
/// (`ComplianceSummary$CompliantResourceCount`: "up to a maximum of 25 for rules").
const RULE_CAP: usize = 25;

/// The bounds `Limit` publishes (`GroupByAPILimit`, `@range(min: 0, max: 1000)`).
pub(crate) const MIN_LIMIT: i64 = 0;
/// See [`MIN_LIMIT`].
pub(crate) const MAX_LIMIT: i64 = 1000;

/// The Region the harness signs its requests for, which is therefore the Region this
/// store's Config data belongs to.
pub(crate) const HOME_REGION: &str = "us-east-1";
/// Another Region, used both as a Region the aggregator does not take in and as the
/// Region a second harness signs for.
pub(crate) const OTHER_REGION: &str = "us-west-2";

/// A source account that is not this emulator's, in the twelve-digit form `AccountId`'s
/// `^\d{12}$` pattern allows.
pub(crate) const OTHER_ACCOUNT: &str = "111122223333";

/// The aggregator every case that is not about the aggregator's own scope uses.
///
/// `pub(crate)` from here down: every suite about an AGGREGATE read of this account's
/// Config data needs exactly this setup — an aggregator over this account's sources and
/// an emulator holding rules with recorded results — so the aggregate suites share one
/// copy of it (`aggregate_compliance_by_config_rules` is the other one).
pub(crate) const AGGREGATOR: &str = "org-wide";

/// This emulator's account id, the account whose Config data the store holds.
pub(crate) fn account() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// One Config rule a row seeds: its name and the evaluation results recorded for it.
pub(crate) type Rule = (String, Vec<&'static str>);

/// `specs` as Config rules: one rule per repetition each spec asks for, carrying the
/// evaluation results that spec names, with names distinct across the whole table row.
pub(crate) fn rules(specs: &[(usize, &[&'static str])]) -> Vec<Rule> {
    let mut seeded = Vec::new();
    for (group, (count, results)) in specs.iter().enumerate() {
        for ordinal in 0..*count {
            seeded.push((format!("rule-{group}-{ordinal}"), results.to_vec()));
        }
    }
    seeded
}

/// An emulator signed for `region`, with a configuration recorder and delivery channel,
/// holding `seeded` Config rules and their recorded evaluation results.
pub(crate) async fn emulator_holding(region: &str, seeded: &[Rule]) -> Emulator {
    let emulator = Emulator::in_region(region);
    with_recorder_and_channel(&emulator).await;
    for (name, results) in seeded {
        emulator.ok("PutConfigRule", &managed_rule(name)).await;
        record_results(&emulator, name, results);
    }
    emulator
}

/// An account aggregation source over `accounts`, aggregating the Regions in `regions`,
/// or every Region when `regions` is empty.
pub(crate) fn account_source(accounts: &[&str], regions: &[&str]) -> Value {
    let mut source = json!({ "AccountIds": accounts });
    if regions.is_empty() {
        source["AllAwsRegions"] = json!(true);
    } else {
        source["AwsRegions"] = json!(regions);
    }
    json!({ "AccountAggregationSources": [source] })
}

/// An organization aggregation source over the Regions in `regions`, or every Region
/// when `regions` is empty.
pub(crate) fn organization_source(regions: &[&str]) -> Value {
    let mut source = json!({ "RoleArn": ROLE_ARN });
    if regions.is_empty() {
        source["AllAwsRegions"] = json!(true);
    } else {
        source["AwsRegions"] = json!(regions);
    }
    json!({ "OrganizationAggregationSource": source })
}

/// Create the aggregator [`AGGREGATOR`] from the `PutConfigurationAggregator` body
/// `sources`.
pub(crate) async fn put_aggregator(emulator: &Emulator, mut sources: Value) {
    sources["ConfigurationAggregatorName"] = json!(AGGREGATOR);
    emulator.ok("PutConfigurationAggregator", &sources).await;
}

/// An aggregator that aggregates this account in every Region.
pub(crate) async fn put_whole_account_aggregator(emulator: &Emulator) {
    put_aggregator(emulator, account_source(&[&account()], &[])).await;
}

/// The `(CappedCount, CapExceeded)` of one reported `ComplianceContributorCount`. A
/// member the response leaves out is the modelled default, which is what a zero count
/// and an unexceeded cap are.
pub(crate) type Contributors = (i64, bool);

/// One reported group: the name it carries and the two counts its `ComplianceSummary`
/// reports.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    name: String,
    compliant: Contributors,
    non_compliant: Contributors,
}

/// Every group a response reports, in the order it carried them.
fn reported(case: &str, body: &Value) -> Vec<Group> {
    let counts = body["AggregateComplianceCounts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected AggregateComplianceCounts, got {body}"));
    counts
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
                compliant: contributors(&summary["CompliantResourceCount"]),
                non_compliant: contributors(&summary["NonCompliantResourceCount"]),
            }
        })
        .collect()
}

/// The counts one `ComplianceContributorCount` reports.
pub(crate) fn contributors(count: &Value) -> Contributors {
    (
        count
            .get("CappedCount")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        count
            .get("CapExceeded")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
    )
}

/// The `GetAggregateConfigRuleComplianceSummary` body for the aggregator every row
/// uses.
fn summary_request() -> Value {
    json!({ "ConfigurationAggregatorName": AGGREGATOR })
}

/// The counts a group reports as `(compliant, noncompliant)`, over a response expected
/// to carry exactly the one group `name`.
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

#[tokio::test]
async fn counts_the_compliant_and_noncompliant_rules_of_the_aggregated_account() {
    struct Case {
        case: &'static str,
        seeded: Vec<Rule>,
        compliant: Contributors,
        non_compliant: Contributors,
    }

    let cases = vec![
        Case {
            case: "a rule nothing has evaluated is neither compliant nor noncompliant",
            seeded: rules(&[(2, &[])]),
            compliant: (0, false),
            non_compliant: (0, false),
        },
        Case {
            case: "every rule compliant",
            seeded: rules(&[(3, &[COMPLIANT])]),
            compliant: (3, false),
            non_compliant: (0, false),
        },
        Case {
            case: "a rule with any noncompliant result counts as noncompliant",
            seeded: rules(&[
                (1, &[COMPLIANT]),
                (1, &[NON_COMPLIANT]),
                (1, &[COMPLIANT, NON_COMPLIANT]),
            ]),
            compliant: (1, false),
            non_compliant: (2, false),
        },
        Case {
            case: "results that place no rule count for neither",
            seeded: rules(&[(1, &[INSUFFICIENT_DATA]), (1, &[NOT_APPLICABLE])]),
            compliant: (0, false),
            non_compliant: (0, false),
        },
        Case {
            case: "many compliant resources still count their rule once",
            seeded: rules(&[(1, &[COMPLIANT, COMPLIANT, COMPLIANT])]),
            compliant: (1, false),
            non_compliant: (0, false),
        },
        Case {
            case: "exactly at the rule cap",
            seeded: rules(&[(RULE_CAP, &[COMPLIANT])]),
            compliant: (RULE_CAP as i64, false),
            non_compliant: (0, false),
        },
        Case {
            case: "one rule past the rule cap, on both counts",
            seeded: rules(&[
                (RULE_CAP + 1, &[COMPLIANT]),
                (RULE_CAP + 2, &[NON_COMPLIANT]),
            ]),
            compliant: (RULE_CAP as i64, true),
            non_compliant: (RULE_CAP as i64, true),
        },
    ];

    for Case {
        case,
        seeded,
        compliant,
        non_compliant,
    } in cases
    {
        let emulator = emulator_holding(HOME_REGION, &seeded).await;
        put_whole_account_aggregator(&emulator).await;

        let body = emulator
            .ok("GetAggregateConfigRuleComplianceSummary", &summary_request())
            .await;
        let group = only_group(case, &body, &account());
        assert_eq!(
            (group.compliant, group.non_compliant),
            (compliant, non_compliant),
            "case {case:?}: expected compliant {compliant:?} and noncompliant \
             {non_compliant:?}, got {group:?}"
        );

        // The summary Config created is timestamped, and the page is complete.
        let timestamp = &body["AggregateComplianceCounts"][0]["ComplianceSummary"]
            ["ComplianceSummaryTimestamp"];
        assert!(
            timestamp.as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: expected a ComplianceSummaryTimestamp, got {timestamp}"
        );
        assert!(
            body.get("NextToken").is_none_or(Value::is_null),
            "case {case:?}: expected no NextToken for a single group, got {body}"
        );

        // The same rules summarised for the account itself: an aggregate read of this
        // account's data and the account's own compliance summary cannot disagree.
        let own = emulator
            .ok("GetComplianceSummaryByConfigRule", &json!({}))
            .await;
        let own_summary = &own["ComplianceSummary"];
        assert_eq!(
            (
                contributors(&own_summary["CompliantResourceCount"]),
                contributors(&own_summary["NonCompliantResourceCount"]),
            ),
            (group.compliant, group.non_compliant),
            "case {case:?}: expected the account's own summary {own_summary} to carry the \
             aggregated counts {group:?}"
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
        let emulator = emulator_holding(region, &rules(&[(1, &[NON_COMPLIANT])])).await;
        put_whole_account_aggregator(&emulator).await;

        let mut request = summary_request();
        if let Some(key) = group_by_key {
            request["GroupByKey"] = json!(key);
        }
        let body = emulator
            .ok("GetAggregateConfigRuleComplianceSummary", &request)
            .await;

        let expected = group_name.map_or_else(account, str::to_string);
        let group = only_group(case, &body, &expected);
        assert_eq!(
            group.non_compliant,
            (1, false),
            "case {case:?}: expected the group's noncompliant count, got {group:?}"
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
        let emulator = emulator_holding(HOME_REGION, &rules(&[(1, &[COMPLIANT])])).await;
        put_aggregator(&emulator, sources).await;

        let mut request = summary_request();
        if let Some(filters) = filters {
            request["Filters"] = filters;
        }
        let body = emulator
            .ok("GetAggregateConfigRuleComplianceSummary", &request)
            .await;
        let groups = reported(case, &body);

        if expected {
            let group = only_group(case, &body, &account);
            assert_eq!(
                group.compliant,
                (1, false),
                "case {case:?}: expected the admitted group's compliant count, got {group:?}"
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
async fn reports_no_group_for_an_aggregated_account_with_no_config_rules() {
    let emulator = emulator_holding(HOME_REGION, &[]).await;
    put_whole_account_aggregator(&emulator).await;

    let body = emulator
        .ok("GetAggregateConfigRuleComplianceSummary", &summary_request())
        .await;
    // Recorded behaviour: a rule-compliance roll-up of an aggregated account holding no
    // rule is an empty result page, not a zero-valued row (unlike the CONFORMANCE PACK
    // summary, which reports a zero-valued row — see
    // `aggregate_conformance_pack_compliance_summary`).
    let groups = reported("no rules to summarise", &body);
    assert!(
        groups.is_empty(),
        "no rules to summarise: expected an empty result page, got {groups:?}"
    );
    assert_eq!(
        body["GroupByKey"].as_str(),
        Some(ACCOUNT_ID),
        "no rules to summarise: expected the GroupByKey to be reported, got {body}"
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

    for (case, limit, served) in cases {
        let emulator = emulator_holding(HOME_REGION, &rules(&[(1, &[COMPLIANT])])).await;
        put_whole_account_aggregator(&emulator).await;

        let mut request = summary_request();
        request["Limit"] = json!(limit);
        if served {
            let body = emulator
                .ok("GetAggregateConfigRuleComplianceSummary", &request)
                .await;
            let group = only_group(case, &body, &account());
            assert_eq!(
                group.compliant,
                (1, false),
                "case {case:?}: expected the group's compliant count, got {group:?}"
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
                .refused("GetAggregateConfigRuleComplianceSummary", &request)
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
    let emulator = emulator_holding(HOME_REGION, &rules(&[(1, &[COMPLIANT])])).await;
    put_whole_account_aggregator(&emulator).await;

    // (case, the request, the expected error code)
    let cases: [(&str, Value, &str); 4] = [
        (
            "an aggregator that does not exist",
            json!({ "ConfigurationAggregatorName": "no-such-aggregator" }),
            "NoSuchConfigurationAggregatorException",
        ),
        (
            "a token this operation did not issue",
            json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "NextToken": "not-a-token-this-operation-issued!",
            }),
            "InvalidNextTokenException",
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
            .refused("GetAggregateConfigRuleComplianceSummary", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected HTTP 400 {expected}, got {status} {code}"
        );
    }
}
