// SPDX-License-Identifier: CC0-1.0

//! What `ListConfigurationRecorders` summarises, end to end over the wire.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the emulator binary mounts — so each
//! expectation is about what the handler computes from persisted state.
//!
//! The rule under test: the operation reports one `ConfigurationRecorderSummary` per
//! configuration recorder the store holds, projecting that recorder's persisted `arn`,
//! `name`, `servicePrincipal`, `recordingScope` and `provider`, in recorder-name order,
//! narrowed to the recording scopes the `recordingScope` filters list and cut into pages
//! of `MaxResults` — handing back a `NextToken` exactly when recorders remain, and
//! refusing a filter or a token this API cannot serve with the modelled
//! `ValidationException`.
//!
//! `PutConfigurationRecorder` creates the ONE customer managed recorder an account can
//! have per Region, and `PutServiceLinkedConfigurationRecorder` creates one
//! service-linked recorder per linked service (while
//! `PutThirdPartyServiceLinkedConfigurationRecorder` is still a scaffold), so the cases
//! that need several recorders of a given shape seed them in the shared engine the
//! router hands the handlers, the way the handlers reach it. The round-trip case goes
//! through the API.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::{
    mint_recorder_id, recorder_arn, service_state, RecorderEntry, TagSet,
    RECORDING_SCOPE_INTERNAL, RECORDING_SCOPE_PAID,
};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// The most recorder summaries one page carries: the maximum of this operation's own
/// `MaxResults` member, from `smithy_query(query="operation",
/// operation="ListConfigurationRecorders")` (`MaxResults`, range 0..20).
const MAX_RESULTS: usize = 20;

/// The only `ConfigurationRecorderFilterName` the model declares, from
/// `smithy_query(query="shape", shape="ConfigurationRecorderFilterName")`:
/// `recordingScope`.
const FILTER_RECORDING_SCOPE: &str = "recordingScope";

/// The only published `Provider` value, from `smithy_query(query="shape",
/// shape="Provider")`: `AZURE`.
const PROVIDER_AZURE: &str = "AZURE";

/// One configuration recorder to place in the store.
#[derive(Debug, Clone)]
struct Seed {
    name: String,
    recording_scope: String,
    service_principal: Option<String>,
    provider: Option<String>,
}

impl Seed {
    /// A recorder of the kind `PutConfigurationRecorder` creates: the account's own,
    /// recording in the `PAID` scope and linked to no service.
    fn customer_managed(name: &str) -> Self {
        Self {
            name: name.to_string(),
            recording_scope: RECORDING_SCOPE_PAID.to_string(),
            service_principal: None,
            provider: None,
        }
    }

    /// A recorder linked to an Amazon Web Services service, recording in the `INTERNAL`
    /// scope.
    fn service_linked(name: &str, service_principal: &str) -> Self {
        Self {
            name: name.to_string(),
            recording_scope: RECORDING_SCOPE_INTERNAL.to_string(),
            service_principal: Some(service_principal.to_string()),
            provider: None,
        }
    }

    /// A service-linked recorder that records a third-party cloud's resources, so it
    /// also carries a `provider`.
    fn third_party(name: &str, service_principal: &str, provider: &str) -> Self {
        Self {
            provider: Some(provider.to_string()),
            ..Self::service_linked(name, service_principal)
        }
    }
}

/// Place `seeds` in the store as this account's configuration recorders, reaching the
/// store the way a handler does, and answer the summaries they must be reported as, in
/// the order they must come back in.
fn seed(emulator: &Emulator, seeds: &[Seed]) -> Vec<Value> {
    let state = service_state(&*emulator.engine).expect("the test engine is the shared engine");
    let mut store = state.lock();
    let mut summaries: Vec<(String, Value)> = Vec::new();
    for seed in seeds {
        let id = mint_recorder_id();
        let arn = recorder_arn(&emulator.region, &seed.name, &id);
        store.recorders.insert(
            seed.name.clone(),
            RecorderEntry {
                name: seed.name.clone(),
                id,
                arn: arn.clone(),
                role_arn: Some(ROLE_ARN.to_string()),
                recording_group: None,
                recording_mode: None,
                recording_scope: Some(seed.recording_scope.clone()),
                service_principal: seed.service_principal.clone(),
                connector_arn: None,
                scope_configuration: None,
                provider: seed.provider.clone(),
                tags: TagSet::new(),
            },
        );

        let mut summary = json!({
            "arn": arn,
            "name": seed.name,
            "recordingScope": seed.recording_scope,
        });
        if let Some(service_principal) = seed.service_principal.as_deref() {
            summary["servicePrincipal"] = json!(service_principal);
        }
        if let Some(provider) = seed.provider.as_deref() {
            summary["provider"] = json!(provider);
        }
        summaries.push((seed.name.clone(), summary));
    }
    summaries.sort_by(|left, right| left.0.cmp(&right.0));
    summaries.into_iter().map(|(_, summary)| summary).collect()
}

/// `count` recorders whose names do NOT ascend with the insertion order, with the
/// recording scope alternating between the two published ones.
fn generated(count: usize) -> Vec<Seed> {
    (0..count)
        .map(|index| {
            let name = format!("recorder-{:03}", 900 - index * 3);
            if index % 2 == 0 {
                Seed::customer_managed(&name)
            } else {
                Seed::service_linked(&name, "ssm.amazonaws.com")
            }
        })
        .collect()
}

/// The request body for one `ListConfigurationRecorders` page.
fn page_request(max_results: Option<i64>, filters: Option<&Value>, token: Option<&str>) -> Value {
    let mut request = json!({});
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(filters) = filters {
        request["Filters"] = filters.clone();
    }
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    request
}

/// The summaries of one `ListConfigurationRecorders` response.
fn summaries_of(case: &str, body: &Value) -> Vec<Value> {
    body["ConfigurationRecorderSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a ConfigurationRecorderSummaries list, got {body}")
        })
        .clone()
}

/// Every page at `max_results`, walked through its `NextToken`: the summaries in order,
/// and the size of each page.
async fn walk_pages(
    case: &str,
    emulator: &Emulator,
    max_results: Option<i64>,
    filters: Option<&Value>,
) -> (Vec<Value>, Vec<usize>) {
    let mut reported: Vec<Value> = Vec::new();
    let mut page_sizes: Vec<usize> = Vec::new();
    let mut next_token: Option<String> = None;
    // A page carries at least one summary whenever a recorder remains, so the walk
    // cannot take more steps than there are recorders, plus the final empty-tail page.
    for _ in 0..=(MAX_RESULTS * 4) {
        let request = page_request(max_results, filters, next_token.as_deref());
        let body = emulator.ok("ListConfigurationRecorders", &request).await;
        let page = summaries_of(case, &body);
        page_sizes.push(page.len());
        reported.extend(page);
        next_token = match body.get("NextToken") {
            Some(Value::String(token)) => {
                assert!(
                    !token.is_empty(),
                    "case {case:?}: expected an opaque NextToken, got {token:?}"
                );
                Some(token.clone())
            }
            _ => None,
        };
        if next_token.is_none() {
            return (reported, page_sizes);
        }
    }
    panic!("case {case:?}: the pages of a finite store must end, got {page_sizes:?}");
}

#[tokio::test]
async fn recorders_are_summarised_from_state_in_name_order_and_paged_at_max_results() {
    // Rows differ along what the rule generalises over: how many recorders the account
    // holds, their kind (customer managed, service-linked, third-party service-linked),
    // whether their names ascend with insertion order, and the counts at and one past
    // one page — crossed with the page size asked for (absent, the member's own default
    // of 0, one, three, and the maximum).
    let seeds: [(&str, Vec<Seed>); 5] = [
        ("no recorders at all", Vec::new()),
        (
            "one customer managed recorder",
            vec![Seed::customer_managed("default")],
        ),
        (
            "three recorders of three kinds, inserted out of name order",
            vec![
                Seed::service_linked(
                    "AWSConfigurationRecorderForSecurityHub",
                    "securityhub.amazonaws.com",
                ),
                Seed::customer_managed("default"),
                Seed::third_party(
                    "AWSConfigurationRecorderForAzure",
                    "azure.partner.amazonaws.com",
                    PROVIDER_AZURE,
                ),
            ],
        ),
        ("exactly one page of recorders", generated(MAX_RESULTS)),
        ("one recorder past one page", generated(MAX_RESULTS + 1)),
    ];

    for (seed_case, recorders) in seeds {
        for max_results in [None, Some(0_i64), Some(1), Some(3), Some(20)] {
            let case = format!("{seed_case}, MaxResults {max_results:?}");
            let emulator = Emulator::new();
            let want = seed(&emulator, &recorders);

            let (reported, page_sizes) = walk_pages(&case, &emulator, max_results, None).await;
            assert_eq!(
                reported,
                want,
                "case {case:?}: expected the {} seeded recorders summarised in name order, \
                 got {reported:?}",
                want.len()
            );

            let page_size = match max_results {
                None | Some(0) => MAX_RESULTS,
                Some(max_results) => usize::try_from(max_results).unwrap_or(MAX_RESULTS),
            };
            for (index, size) in page_sizes.iter().enumerate() {
                assert!(
                    *size <= page_size,
                    "case {case:?}: page {index} must carry at most {page_size} summaries, \
                     got {size}"
                );
            }
            // Every page but the last is full: a NextToken comes back only when
            // recorders remain.
            for (index, size) in page_sizes.iter().enumerate().take(page_sizes.len() - 1) {
                assert_eq!(
                    *size, page_size,
                    "case {case:?}: page {index} must be full when a NextToken follows it, \
                     got {size}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_recorder_created_through_the_api_is_summarised_with_what_describe_reports() {
    // Rows differ along the recorder the request asks for: whether it names the recorder
    // (an absent name is assigned "default") and which of the two published
    // RecordingScope values it records in (absent means the PAID scope a customer
    // managed recorder is billed for).
    let cases: [(&str, Value, &str, &str); 5] = [
        (
            "no name and no recording scope",
            json!({ "roleARN": ROLE_ARN }),
            "default",
            RECORDING_SCOPE_PAID,
        ),
        (
            "a named recorder",
            json!({ "name": "primary", "roleARN": ROLE_ARN }),
            "primary",
            RECORDING_SCOPE_PAID,
        ),
        (
            "the PAID recording scope, asked for",
            json!({
                "name": "billed-recorder",
                "roleARN": ROLE_ARN,
                "recordingScope": RECORDING_SCOPE_PAID,
            }),
            "billed-recorder",
            RECORDING_SCOPE_PAID,
        ),
        (
            "the INTERNAL recording scope, asked for",
            json!({
                "name": "free-recorder",
                "roleARN": ROLE_ARN,
                "recordingScope": RECORDING_SCOPE_INTERNAL,
            }),
            "free-recorder",
            RECORDING_SCOPE_INTERNAL,
        ),
        (
            "a name with the punctuation a recorder name allows",
            json!({ "name": "team.config-recorder_1", "roleARN": ROLE_ARN }),
            "team.config-recorder_1",
            RECORDING_SCOPE_PAID,
        ),
    ];

    for (case, recorder, name, recording_scope) in cases {
        let emulator = Emulator::new();
        emulator
            .ok("PutConfigurationRecorder", &recorder_request(recorder))
            .await;

        // The ARN the summary carries is the one the recorder was created with, which is
        // what DescribeConfigurationRecorders reports for the same recorder.
        let described = emulator
            .ok("DescribeConfigurationRecorders", &json!({}))
            .await;
        let arn = described["ConfigurationRecorders"][0]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a described recorder, got {described}"))
            .to_string();

        let body = emulator
            .ok("ListConfigurationRecorders", &json!({}))
            .await;
        let reported = summaries_of(case, &body);
        assert_eq!(
            reported,
            vec![json!({ "arn": arn, "name": name, "recordingScope": recording_scope })],
            "case {case:?}: expected the created recorder summarised from state, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn filters_narrow_the_summaries_to_the_recording_scopes_they_list() {
    let recorders = vec![
        Seed::customer_managed("default"),
        Seed::service_linked(
            "AWSConfigurationRecorderForSecurityHub",
            "securityhub.amazonaws.com",
        ),
        Seed::third_party(
            "AWSConfigurationRecorderForAzure",
            "azure.partner.amazonaws.com",
            PROVIDER_AZURE,
        ),
    ];
    let paid = ["default"];
    let internal = [
        "AWSConfigurationRecorderForAzure",
        "AWSConfigurationRecorderForSecurityHub",
    ];
    let all = [
        "AWSConfigurationRecorderForAzure",
        "AWSConfigurationRecorderForSecurityHub",
        "default",
    ];

    // Rows differ along the filters: none, each published scope on its own, both scopes
    // in one filter (which selects either), two filters that narrow each other to
    // nothing, and a filter that lists no value at all.
    let cases: [(&str, Option<Value>, Vec<&str>); 6] = [
        ("no filters", None, all.to_vec()),
        (
            "the PAID scope",
            Some(json!([{ "filterName": FILTER_RECORDING_SCOPE, "filterValue": [RECORDING_SCOPE_PAID] }])),
            paid.to_vec(),
        ),
        (
            "the INTERNAL scope",
            Some(json!([{ "filterName": FILTER_RECORDING_SCOPE, "filterValue": [RECORDING_SCOPE_INTERNAL] }])),
            internal.to_vec(),
        ),
        (
            "both scopes in one filter",
            Some(json!([{
                "filterName": FILTER_RECORDING_SCOPE,
                "filterValue": [RECORDING_SCOPE_INTERNAL, RECORDING_SCOPE_PAID],
            }])),
            all.to_vec(),
        ),
        (
            "two filters that narrow each other",
            Some(json!([
                { "filterName": FILTER_RECORDING_SCOPE, "filterValue": [RECORDING_SCOPE_INTERNAL] },
                { "filterName": FILTER_RECORDING_SCOPE, "filterValue": [RECORDING_SCOPE_PAID] },
            ])),
            Vec::new(),
        ),
        (
            "a filter that lists no value",
            Some(json!([{ "filterName": FILTER_RECORDING_SCOPE, "filterValue": [] }])),
            all.to_vec(),
        ),
    ];

    for (case, filters, want_names) in cases {
        let emulator = Emulator::new();
        let seeded = seed(&emulator, &recorders);

        let (reported, _) = walk_pages(case, &emulator, None, filters.as_ref()).await;
        let names: Vec<&str> = reported
            .iter()
            .map(|summary| summary["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names, want_names,
            "case {case:?}: expected the recorders in those scopes, got {reported:?}"
        );
        // A narrowed answer still projects every member from state.
        for summary in &reported {
            assert!(
                seeded.contains(summary),
                "case {case:?}: expected a summary read out of state, got {summary}"
            );
        }
    }
}

#[tokio::test]
async fn a_filter_or_a_token_this_operation_cannot_serve_is_refused() {
    // Rows differ along the form of the bad request: a filter naming a member this API
    // does not filter on, a filter with no name, a value that names no recording scope
    // (including one that differs only in case), a token this operation did not issue,
    // and a page size one past each published bound of the modelled 0..20 range.
    let cases: [(&str, Value); 7] = [
        (
            "a filter on another member",
            json!({ "Filters": [{ "filterName": "recordingMode", "filterValue": [RECORDING_SCOPE_PAID] }] }),
        ),
        (
            "a filter with no name",
            json!({ "Filters": [{ "filterValue": [RECORDING_SCOPE_PAID] }] }),
        ),
        (
            "a value that is no recording scope",
            json!({ "Filters": [{ "filterName": FILTER_RECORDING_SCOPE, "filterValue": ["EXTERNAL"] }] }),
        ),
        (
            "a recording scope in the wrong case",
            json!({ "Filters": [{ "filterName": FILTER_RECORDING_SCOPE, "filterValue": ["paid"] }] }),
        ),
        (
            "a NextToken this operation did not issue",
            json!({ "NextToken": "not-a-token!!" }),
        ),
        ("MaxResults one below the range", json!({ "MaxResults": -1 })),
        (
            "MaxResults one above the range",
            json!({ "MaxResults": i64::try_from(MAX_RESULTS).unwrap_or(20) + 1 }),
        ),
    ];

    for (case, request) in cases {
        let emulator = Emulator::new();
        let want = seed(
            &emulator,
            &[
                Seed::customer_managed("default"),
                Seed::service_linked("AWSConfigurationRecorderForSsm", "ssm.amazonaws.com"),
            ],
        );

        let (status, code) = emulator
            .refused("ListConfigurationRecorders", &request)
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code}"
        );

        // The refusal changes nothing: every recorder is still summarised.
        let (reported, _) = walk_pages(case, &emulator, None, None).await;
        assert_eq!(
            reported, want,
            "case {case:?}: the refused request must leave the store alone, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn listing_recorders_reports_state_without_changing_it() {
    // Rows differ along how many recorders the store holds and the page size, so the
    // read is checked both when it pages and when it does not.
    for (seed_case, count, max_results) in [
        ("an empty store", 0_usize, None),
        ("a single recorder", 1, None),
        ("several recorders on one page", 5, None),
        ("several recorders over three pages", 5, Some(2_i64)),
    ] {
        let case = format!("{seed_case}, MaxResults {max_results:?}");
        let emulator = Emulator::new();
        let recorders = generated(count);
        let want = seed(&emulator, &recorders);

        let (first, _) = walk_pages(&case, &emulator, max_results, None).await;
        let described = emulator
            .ok("DescribeConfigurationRecorders", &json!({}))
            .await;
        let (again, _) = walk_pages(&case, &emulator, max_results, None).await;

        assert_eq!(
            first, want,
            "case {case:?}: expected the seeded recorders, got {first:?}"
        );
        assert_eq!(
            again, first,
            "case {case:?}: a second read must report the same summaries, got {again:?}"
        );
        assert_eq!(
            described["ConfigurationRecorders"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            count,
            "case {case:?}: listing must leave the recorders describable, got {described}"
        );
    }
}
