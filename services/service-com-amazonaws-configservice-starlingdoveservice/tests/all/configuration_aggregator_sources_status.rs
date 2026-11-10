// SPDX-License-Identifier: CC0-1.0

//! `DescribeConfigurationAggregatorSourcesStatus` end to end over the wire.
//!
//! The rule pinned here, over a TABLE of inputs rather than one case:
//! `DescribeConfigurationAggregatorSourcesStatus` reports one sync-status record per
//! (source, Region) pair the named aggregator's own aggregation sources enumerate —
//! the source id and source type of the source, the Region it is collected from, and
//! the status and instant of the last verification — narrowed to the status types the
//! request filters on and served a page at a time.
//!
//! The rows differ along the FORM of the aggregation source (which kind it is, how
//! many accounts it names, how many Regions, whether it aggregates every Region,
//! whether two sources overlap), along the Region the request is signed for, along the
//! status filter, and along the page size asked for.
//!
//! The harness (`Emulator`) is the one `config_setup_and_rule_loop` already builds;
//! every case drives the emulator the way a client does, through the
//! `MultiServiceRouter`.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    service_state, AGGREGATION_COMPLETES_AFTER,
};

use crate::config_setup_and_rule_loop::{Emulator, ROLE_ARN};

/// Twelve-digit source account ids, the form `AccountId`'s `^\d{12}$` pattern allows.
const ACCOUNT_A: &str = "111122223333";
/// See [`ACCOUNT_A`].
const ACCOUNT_B: &str = "444455556666";
/// See [`ACCOUNT_A`].
const ACCOUNT_C: &str = "777788889999";

/// Every published `AggregatedSourceType` value, from
/// `smithy_query(query="shape", shape="AggregatedSourceType")`: `ACCOUNT` and
/// `ORGANIZATION`.
const SOURCE_TYPE_ACCOUNT: &str = "ACCOUNT";
/// See [`SOURCE_TYPE_ACCOUNT`].
const SOURCE_TYPE_ORGANIZATION: &str = "ORGANIZATION";

/// Every published `AggregatedSourceStatusType` value, from
/// `smithy_query(query="shape", shape="AggregatedSourceStatusType")`: `FAILED`,
/// `SUCCEEDED` and `OUTDATED`.
const STATUS_FAILED: &str = "FAILED";
/// See [`STATUS_FAILED`].
const STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// See [`STATUS_FAILED`].
const STATUS_OUTDATED: &str = "OUTDATED";

/// Stands for the account this emulator serves, in an expected source id: an
/// organization aggregation source is named by that account, which the test only knows
/// at run time.
const THIS_ACCOUNT: &str = "<this account>";

/// Stands for the Region the request is signed for, in an expected Region: a source
/// that aggregates every Region is collected from the one Region this store stands for.
const THIS_REGION: &str = "<this region>";

/// One source a case expects a status for: its source id, its source type and the
/// Region it is collected from, where the id and the Region may be a placeholder
/// ([`THIS_ACCOUNT`], [`THIS_REGION`]).
type ExpectedSource = (&'static str, &'static str, &'static str);

/// One row of a source-status case: what it is called, the aggregation sources the
/// aggregator is created with, and the sources it must then report.
type SourceCase = (&'static str, Value, &'static [ExpectedSource]);

/// The (source id, source type, Region) one reported status names.
type Triple = (String, String, String);

/// An account aggregation source over `accounts`, aggregating the Regions in
/// `regions`, or every Region when `regions` is empty.
fn account_source(accounts: &[&str], regions: &[&str]) -> Value {
    let mut source = json!({ "AccountIds": accounts });
    if regions.is_empty() {
        source["AllAwsRegions"] = json!(true);
    } else {
        source["AwsRegions"] = json!(regions);
    }
    source
}

/// An organization aggregation source over the Regions in `regions`, or every Region
/// when `regions` is empty.
fn organization_source(regions: &[&str]) -> Value {
    let mut source = json!({ "RoleArn": ROLE_ARN });
    if regions.is_empty() {
        source["AllAwsRegions"] = json!(true);
    } else {
        source["AwsRegions"] = json!(regions);
    }
    source
}

/// Create `name` with the aggregation sources `request` carries, answering the
/// `LastUpdatedTime` Config recorded for it.
///
/// Writing an aggregator's sources STARTS the aggregation of those sources, which
/// completes a span later ([`AGGREGATION_COMPLETES_AFTER`]) — until then the aggregator
/// has no source status to report at all, which is what
/// `an_aggregator_reports_no_source_status_until_it_has_aggregated` pins. Every case
/// here is about the statuses of an aggregator that HAS aggregated, so the sources are
/// moved that span into the past rather than waited out, leaving the instant the
/// aggregation completed equal to the `LastUpdatedTime` the create reported.
async fn put_aggregator(emulator: &Emulator, name: &str, request: &Value) -> f64 {
    let mut body = request.clone();
    body["ConfigurationAggregatorName"] = json!(name);
    let created = emulator.ok("PutConfigurationAggregator", &body).await;
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    let aggregator = store
        .configuration_aggregators
        .get_mut(name)
        .unwrap_or_else(|| panic!("case {name:?}: the created aggregator must be in state"));
    aggregator.last_updated_at -= AGGREGATION_COMPLETES_AFTER;
    drop(store);
    created["ConfigurationAggregator"]["LastUpdatedTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {name:?}: the aggregator must carry a LastUpdatedTime, got {created}"))
}

/// The page of statuses `request` is answered with, and the `NextToken` beside it.
async fn sources_status(emulator: &Emulator, request: &Value) -> (Vec<Value>, Option<String>) {
    let response = emulator
        .ok("DescribeConfigurationAggregatorSourcesStatus", request)
        .await;
    let page = response["AggregatedSourceStatusList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {request}: expected a status list, got {response}"))
        .clone();
    let next = response
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (page, next)
}

/// The (source id, source type, Region) a reported status names.
fn triple(status: &Value, case: &str) -> Triple {
    let member = |key: &str| {
        status[key]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected {key} on the status, got {status}"))
            .to_string()
    };
    (member("SourceId"), member("SourceType"), member("AwsRegion"))
}

/// The triples `expected` names, with the [`THIS_ACCOUNT`] and [`THIS_REGION`]
/// placeholders resolved, in the (source id, Region) order the statuses are served in.
fn expected_triples(
    expected: &[ExpectedSource],
    account_id: &str,
    region: &str,
) -> Vec<Triple> {
    let resolve = |value: &str, placeholder: &str, actual: &str| {
        if value == placeholder {
            actual.to_string()
        } else {
            value.to_string()
        }
    };
    let mut triples: Vec<Triple> = expected
        .iter()
        .map(|(source_id, source_type, aws_region)| {
            (
                resolve(source_id, THIS_ACCOUNT, account_id),
                (*source_type).to_string(),
                resolve(aws_region, THIS_REGION, region),
            )
        })
        .collect();
    triples.sort_by(|left, right| (&left.0, &left.2).cmp(&(&right.0, &right.2)));
    triples
}

#[tokio::test]
async fn every_configured_source_and_region_is_reported_exactly_once() {
    // Rows differ along the FORM of the aggregation sources: the kind, the number of
    // accounts and Regions, every Region instead of a listed set, and two sources that
    // name the same account in the same Region.
    let cases: [SourceCase; 8] = [
        (
            "one account, one region",
            json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])] }),
            &[(ACCOUNT_A, SOURCE_TYPE_ACCOUNT, "us-east-1")],
        ),
        (
            "one account, three regions",
            json!({
                "AccountAggregationSources": [
                    account_source(&[ACCOUNT_A], &["eu-west-2", "us-east-1", "ap-south-1"]),
                ],
            }),
            &[
                (ACCOUNT_A, SOURCE_TYPE_ACCOUNT, "ap-south-1"),
                (ACCOUNT_A, SOURCE_TYPE_ACCOUNT, "eu-west-2"),
                (ACCOUNT_A, SOURCE_TYPE_ACCOUNT, "us-east-1"),
            ],
        ),
        (
            "three accounts, two regions",
            json!({
                "AccountAggregationSources": [
                    account_source(&[ACCOUNT_C, ACCOUNT_A, ACCOUNT_B], &["us-west-2", "us-east-1"]),
                ],
            }),
            &[
                (ACCOUNT_A, SOURCE_TYPE_ACCOUNT, "us-east-1"),
                (ACCOUNT_A, SOURCE_TYPE_ACCOUNT, "us-west-2"),
                (ACCOUNT_B, SOURCE_TYPE_ACCOUNT, "us-east-1"),
                (ACCOUNT_B, SOURCE_TYPE_ACCOUNT, "us-west-2"),
                (ACCOUNT_C, SOURCE_TYPE_ACCOUNT, "us-east-1"),
                (ACCOUNT_C, SOURCE_TYPE_ACCOUNT, "us-west-2"),
            ],
        ),
        (
            "one account, every region",
            json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_B], &[])] }),
            &[(ACCOUNT_B, SOURCE_TYPE_ACCOUNT, THIS_REGION)],
        ),
        (
            "an account and a region each named twice in the one source",
            json!({
                "AccountAggregationSources": [
                    account_source(
                        &[ACCOUNT_A, ACCOUNT_B, ACCOUNT_A],
                        &["us-east-1", "eu-central-1", "us-east-1"],
                    ),
                ],
            }),
            &[
                (ACCOUNT_A, SOURCE_TYPE_ACCOUNT, "eu-central-1"),
                (ACCOUNT_A, SOURCE_TYPE_ACCOUNT, "us-east-1"),
                (ACCOUNT_B, SOURCE_TYPE_ACCOUNT, "eu-central-1"),
                (ACCOUNT_B, SOURCE_TYPE_ACCOUNT, "us-east-1"),
            ],
        ),
        (
            "organization, two regions",
            json!({
                "OrganizationAggregationSource": organization_source(&["us-east-1", "eu-west-1"]),
            }),
            &[
                (THIS_ACCOUNT, SOURCE_TYPE_ORGANIZATION, "eu-west-1"),
                (THIS_ACCOUNT, SOURCE_TYPE_ORGANIZATION, "us-east-1"),
            ],
        ),
        (
            "organization, every region",
            json!({ "OrganizationAggregationSource": organization_source(&[]) }),
            &[(THIS_ACCOUNT, SOURCE_TYPE_ORGANIZATION, THIS_REGION)],
        ),
        (
            "accounts with a resource-type filter, which narrows no source",
            json!({
                "AccountAggregationSources": [account_source(&[ACCOUNT_C], &["us-east-2"])],
                "AggregatorFilters": {
                    "ResourceType": { "Type": "INCLUDE", "Value": ["AWS::S3::Bucket"] },
                },
            }),
            &[(ACCOUNT_C, SOURCE_TYPE_ACCOUNT, "us-east-2")],
        ),
    ];

    let emulator = Emulator::new();
    let account_id = emulator_server::emulator_account_id();
    for (index, (case, sources, expected)) in cases.iter().enumerate() {
        let name = format!("sources-status-{index}");
        let last_updated = put_aggregator(&emulator, &name, sources).await;

        let (page, next) = sources_status(
            &emulator,
            &json!({ "ConfigurationAggregatorName": name }),
        )
        .await;
        assert!(
            next.is_none(),
            "case {case:?}: every source fits one default page, got NextToken {next:?}"
        );

        let actual: Vec<Triple> =
            page.iter().map(|status| triple(status, case)).collect();
        let expected = expected_triples(expected, account_id, &emulator.region);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected one status per configured source and Region, in \
             (source, Region) order, got {actual:?}"
        );

        for status in &page {
            assert_eq!(
                status["LastUpdateStatus"].as_str(),
                Some(STATUS_SUCCEEDED),
                "case {case:?}: expected the configured source to have been aggregated, \
                 got {status}"
            );
            assert_eq!(
                status["LastUpdateTime"].as_f64(),
                Some(last_updated),
                "case {case:?}: expected the verification instant to be when the sources \
                 were last written ({last_updated}), got {status}"
            );
            for error_member in ["LastErrorCode", "LastErrorMessage"] {
                assert!(
                    status.get(error_member).is_none(),
                    "case {case:?}: a source that was aggregated reports no \
                     {error_member}, got {status}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_source_that_aggregates_every_region_is_collected_from_the_signed_region() {
    // Rows differ along the Region the request is signed for, which is the one Region
    // this store stands for and so the one a source over every Region resolves to.
    for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
        let emulator = Emulator::in_region(region);
        let account_id = emulator_server::emulator_account_id();
        let cases: [SourceCase; 2] = [
            (
                "accounts over every region",
                json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_A], &[])] }),
                &[(ACCOUNT_A, SOURCE_TYPE_ACCOUNT, THIS_REGION)],
            ),
            (
                "an organization over every region",
                json!({ "OrganizationAggregationSource": organization_source(&[]) }),
                &[(THIS_ACCOUNT, SOURCE_TYPE_ORGANIZATION, THIS_REGION)],
            ),
        ];
        for (index, (case, sources, expected)) in cases.iter().enumerate() {
            let name = format!("all-regions-{index}");
            put_aggregator(&emulator, &name, sources).await;
            let (page, _) = sources_status(
                &emulator,
                &json!({ "ConfigurationAggregatorName": name }),
            )
            .await;
            let case = format!("{region}/{case}");
            let actual: Vec<Triple> =
                page.iter().map(|status| triple(status, &case)).collect();
            let expected = expected_triples(expected, account_id, region);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the source to be collected from the Region the \
                 request is signed for, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_update_status_filter_selects_the_statuses_it_names() {
    let emulator = Emulator::new();
    // Two accounts in two Regions: four source relationships, every one of them
    // aggregated, so a filter that names SUCCEEDED selects all four and one that names
    // only the failure statuses selects none.
    put_aggregator(
        &emulator,
        "filtered",
        &json!({
            "AccountAggregationSources": [
                account_source(&[ACCOUNT_A, ACCOUNT_B], &["us-east-1", "eu-west-1"]),
            ],
        }),
    )
    .await;

    // Rows differ along WHICH statuses the filter names: none, each single value, and
    // combinations including every published one.
    let cases: [(&str, Option<Vec<&str>>, usize); 7] = [
        ("no filter at all", None, 4),
        ("only SUCCEEDED", Some(vec![STATUS_SUCCEEDED]), 4),
        ("only FAILED", Some(vec![STATUS_FAILED]), 0),
        ("only OUTDATED", Some(vec![STATUS_OUTDATED]), 0),
        (
            "both failure statuses",
            Some(vec![STATUS_FAILED, STATUS_OUTDATED]),
            0,
        ),
        (
            "SUCCEEDED beside a failure status",
            Some(vec![STATUS_OUTDATED, STATUS_SUCCEEDED]),
            4,
        ),
        (
            "every published status",
            Some(vec![STATUS_FAILED, STATUS_SUCCEEDED, STATUS_OUTDATED]),
            4,
        ),
    ];
    for (case, filter, expected_len) in cases {
        let mut request = json!({ "ConfigurationAggregatorName": "filtered" });
        if let Some(filter) = filter.as_ref() {
            request["UpdateStatus"] = json!(filter);
        }
        let (page, _) = sources_status(&emulator, &request).await;
        assert_eq!(
            page.len(),
            expected_len,
            "case {case:?}: expected {expected_len} statuses for filter {filter:?}, got \
             {page:?}"
        );
        for status in &page {
            let reported = status["LastUpdateStatus"].as_str().unwrap_or_default();
            assert!(
                filter
                    .as_ref()
                    .is_none_or(|filter| filter.contains(&reported)),
                "case {case:?}: expected only the filtered statuses, got {status}"
            );
        }
    }

    // A status the published enum does not name is refused at the request boundary
    // rather than answered with an empty page.
    let (status, code) = emulator
        .refused(
            "DescribeConfigurationAggregatorSourcesStatus",
            &json!({
                "ConfigurationAggregatorName": "filtered",
                "UpdateStatus": ["PENDING"],
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"a status no source can be in\": expected HTTP 400 ValidationException for \
         an unpublished UpdateStatus value, got {status} {code}"
    );
}

#[tokio::test]
async fn statuses_are_served_a_page_at_a_time() {
    let emulator = Emulator::new();
    let account_id = emulator_server::emulator_account_id();
    // Three accounts in two Regions: six source relationships to page over.
    put_aggregator(
        &emulator,
        "paged",
        &json!({
            "AccountAggregationSources": [
                account_source(&[ACCOUNT_A, ACCOUNT_B, ACCOUNT_C], &["us-east-1", "eu-west-1"]),
            ],
        }),
    )
    .await;
    let every_triple = expected_triples(
        &[
            (ACCOUNT_A, SOURCE_TYPE_ACCOUNT, "eu-west-1"),
            (ACCOUNT_A, SOURCE_TYPE_ACCOUNT, "us-east-1"),
            (ACCOUNT_B, SOURCE_TYPE_ACCOUNT, "eu-west-1"),
            (ACCOUNT_B, SOURCE_TYPE_ACCOUNT, "us-east-1"),
            (ACCOUNT_C, SOURCE_TYPE_ACCOUNT, "eu-west-1"),
            (ACCOUNT_C, SOURCE_TYPE_ACCOUNT, "us-east-1"),
        ],
        account_id,
        &emulator.region,
    );

    // Rows differ along the page size asked for: the modelled `Limit` default of 0
    // ("the default is maximum"), each small size, exactly what is in state and one
    // past it.
    let cases: [(&str, i64, &[usize]); 6] = [
        ("the default page size", 0, &[6]),
        ("one per page", 1, &[1, 1, 1, 1, 1, 1]),
        ("two per page", 2, &[2, 2, 2]),
        ("four per page", 4, &[4, 2]),
        ("exactly what is in state", 6, &[6]),
        ("more than is in state", 7, &[6]),
    ];
    for (case, limit, expected_pages) in cases {
        let mut next: Option<String> = None;
        let mut seen: Vec<Triple> = Vec::new();
        for (index, expected_len) in expected_pages.iter().enumerate() {
            let mut request = json!({
                "ConfigurationAggregatorName": "paged",
                "Limit": limit,
            });
            if let Some(token) = next.as_deref() {
                request["NextToken"] = json!(token);
            }
            let (page, token) = sources_status(&emulator, &request).await;
            assert_eq!(
                page.len(),
                *expected_len,
                "case {case:?}: expected {expected_len} statuses on page {index}, got {page:?}"
            );
            seen.extend(page.iter().map(|status| triple(status, case)));
            next = token;
            let last_page = index + 1 == expected_pages.len();
            assert_eq!(
                next.is_none(),
                last_page,
                "case {case:?}: page {index} carried NextToken {next:?}"
            );
        }
        assert_eq!(
            seen, every_triple,
            "case {case:?}: expected every source exactly once in (source, Region) order, \
             got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_request_that_names_no_aggregator_in_state_is_refused() {
    let emulator = Emulator::new();
    put_aggregator(
        &emulator,
        "present",
        &json!({ "AccountAggregationSources": [account_source(&[ACCOUNT_A], &["us-east-1"])] }),
    )
    .await;

    // Rows differ along WHAT cannot be served: an aggregator no name holds, a token
    // this operation never issued, a name outside the modelled pattern and a page size
    // past the modelled range.
    let cases: [(&str, Value, &[&str]); 5] = [
        (
            "an aggregator that does not exist",
            json!({ "ConfigurationAggregatorName": "absent" }),
            &["NoSuchConfigurationAggregatorException"],
        ),
        (
            "a token this operation never issued",
            json!({ "ConfigurationAggregatorName": "present", "NextToken": "not-a-token-!!" }),
            &["InvalidNextTokenException"],
        ),
        (
            "a token that decodes to no source pair",
            json!({
                "ConfigurationAggregatorName": "present",
                // base64 of "nonsense", which is not a (source id, Region) pair.
                "NextToken": "bm9uc2Vuc2U=",
            }),
            &["InvalidNextTokenException"],
        ),
        (
            "a name outside the modelled pattern",
            json!({ "ConfigurationAggregatorName": "not a valid name" }),
            &["ValidationException"],
        ),
        (
            "a page size past the modelled range",
            json!({ "ConfigurationAggregatorName": "present", "Limit": 101 }),
            &["ValidationException", "InvalidLimitException"],
        ),
    ];
    for (case, request, expected_codes) in cases {
        let (status, code) = emulator
            .refused("DescribeConfigurationAggregatorSourcesStatus", &request)
            .await;
        assert!(
            status == 400 && expected_codes.contains(&code.as_str()),
            "case {case:?}: expected HTTP 400 one of {expected_codes:?}, got {status} {code}"
        );
    }
}

/// The most accounts one aggregation source may name, and the most Regions it may
/// list, which `PutConfigurationAggregator` holds a stored source to: the pairs a
/// status read walks are the product of the two, so each is bounded where it is
/// written. Mirrors `crate::state::AGGREGATION_SOURCE_ACCOUNTS_QUOTA` and
/// `AGGREGATION_SOURCE_REGIONS_QUOTA`.
const ACCOUNTS_BOUND: usize = 10_000;
/// See [`ACCOUNTS_BOUND`].
const REGIONS_BOUND: usize = 100;

/// `count` distinct twelve-digit account ids (`AccountId`, pattern `^\d{12}$`).
fn many_accounts(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{:012}", index + 1)).collect()
}

/// `count` distinct Region names, each within `AwsRegion`'s 1..64 length.
fn many_regions(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("eu-test-{index:04}")).collect()
}

#[tokio::test]
async fn an_aggregation_source_is_bounded_where_it_is_written() {
    let emulator = Emulator::new();

    // Rows differ along WHICH list is at or past its bound, and by which kind of
    // source carries it: a source AT the bound is stored and read back, one PAST it is
    // refused and stores nothing.
    let accepted: [(&str, Value, usize); 2] = [
        (
            "accounts at the bound",
            json!({
                "AccountAggregationSources": [{
                    "AccountIds": many_accounts(ACCOUNTS_BOUND),
                    "AwsRegions": ["us-east-1"],
                }],
            }),
            ACCOUNTS_BOUND,
        ),
        (
            "regions at the bound",
            json!({
                "AccountAggregationSources": [{
                    "AccountIds": [ACCOUNT_A],
                    "AwsRegions": many_regions(REGIONS_BOUND),
                }],
            }),
            REGIONS_BOUND,
        ),
    ];
    for (index, (case, sources, expected_pairs)) in accepted.iter().enumerate() {
        let name = format!("bounded-{index}");
        put_aggregator(&emulator, &name, sources).await;
        // The read serves ONE page of the pairs that source holds, whatever their
        // number: the default page is the modelled maximum of 100.
        let (page, next) = sources_status(
            &emulator,
            &json!({ "ConfigurationAggregatorName": name }),
        )
        .await;
        let expected_len = (*expected_pairs).min(100);
        assert_eq!(
            page.len(),
            expected_len,
            "case {case:?}: expected a page of {expected_len} statuses, got {}",
            page.len()
        );
        assert_eq!(
            next.is_some(),
            *expected_pairs > expected_len,
            "case {case:?}: expected a NextToken only with pairs left to serve, got {next:?}"
        );
        // A filter naming a status none of those sources is in is answered with an
        // empty page, without walking them.
        let (filtered, next) = sources_status(
            &emulator,
            &json!({
                "ConfigurationAggregatorName": name,
                "UpdateStatus": [STATUS_FAILED, STATUS_OUTDATED],
            }),
        )
        .await;
        assert!(
            filtered.is_empty() && next.is_none(),
            "case {case:?}: expected no statuses for a filter the sources are not in, got \
             {filtered:?} with {next:?}"
        );
    }

    let refused: [(&str, Value, &str); 3] = [
        (
            "one account past the bound",
            json!({
                "ConfigurationAggregatorName": "past-accounts",
                "AccountAggregationSources": [{
                    "AccountIds": many_accounts(ACCOUNTS_BOUND + 1),
                    "AwsRegions": ["us-east-1"],
                }],
            }),
            "LimitExceededException",
        ),
        (
            "one region past the bound",
            json!({
                "ConfigurationAggregatorName": "past-regions",
                "AccountAggregationSources": [{
                    "AccountIds": [ACCOUNT_A],
                    "AwsRegions": many_regions(REGIONS_BOUND + 1),
                }],
            }),
            "InvalidParameterValueException",
        ),
        (
            "an organization one region past the bound",
            json!({
                "ConfigurationAggregatorName": "past-org-regions",
                "OrganizationAggregationSource": {
                    "RoleArn": ROLE_ARN,
                    "AwsRegions": many_regions(REGIONS_BOUND + 1),
                },
            }),
            "InvalidParameterValueException",
        ),
    ];
    for (case, request, expected_code) in refused {
        let (status, code) = emulator
            .refused("PutConfigurationAggregator", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected HTTP 400 {expected_code}, got {status} {code}"
        );
        let name = request["ConfigurationAggregatorName"]
            .as_str()
            .unwrap_or_default();
        let (status, code) = emulator
            .refused(
                "DescribeConfigurationAggregatorSourcesStatus",
                &json!({ "ConfigurationAggregatorName": name }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchConfigurationAggregatorException"),
            "case {case:?}: a refused put stores no aggregator to report sources for, got \
             {status} {code}"
        );
    }
}
