// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `ListTrialComponents`.
//!
//! Every case goes over the wire (`POST /` with
//! `X-Amz-Target: SageMaker.ListTrialComponents`, `awsJson1_1`) through the same
//! `MultiServiceRouter` the production router uses, so what these tests exercise is what a
//! real SDK client gets. The harness is reused from `inference_deployment_lifecycle`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! member of the modelled `SortTrialComponentsBy` × `SortOrder` grid and both documented
//! defaults, both creation-time bounds, each of the three mutually exclusive entity
//! filters, every page size from one entry to past the published `MaxResults` bound and
//! past the service's own ceiling, and the refusal of inputs outside the documented value
//! sets. Each summary is checked against what `DescribeTrialComponent` reads back, so the
//! listing reports stored state rather than anything the request carried.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::{
    SORT_ORDER_VALUES, SORT_TRIAL_COMPONENTS_BY_VALUES,
};
use service_com_amazonaws_sagemaker_sagemaker::state::{entity_key, MAX_PAGE_SIZE};
use service_com_amazonaws_sagemaker_sagemaker::ComAmazonawsSagemakerSagemakerState;

use crate::inference_deployment_lifecycle::{error_code, str_field, Emulator, CREATE_SPACING};

/// The service's own server-side page ceiling, read from the service rather than
/// restated, so changing it does not fail this test for an unrelated reason.
const PAGE_CEILING: usize = MAX_PAGE_SIZE as usize;

/// `MaxResults`' documented default for `ListTrialComponents` ("The maximum number of
/// components to return in the response. The default value is 10.", smithy_query
/// operation=ListTrialComponents).
const DOCUMENTED_DEFAULT_MAX_RESULTS: usize = 10;

/// One seeded trial component: the body it was created from and the ARN the service minted.
struct Seeded {
    case: &'static str,
    name: String,
    arn: String,
    created: f64,
}

/// The create bodies the sort, filter and paging tables range over, created in this order.
///
/// The names are deliberately NOT in creation order, so a `Name` sort and a `CreationTime`
/// sort are distinguishable. Rows differ along what the rules generalise over: whether
/// `DisplayName`, `Status`, `StartTime`/`EndTime`, `Parameters` and the artifact maps are
/// sent, and which parameter-union arm a parameter carries.
fn seed_bodies() -> Vec<(&'static str, Value)> {
    vec![
        (
            "bare",
            json!({ "TrialComponentName": "tc-gamma" }),
        ),
        (
            "status-and-times",
            json!({
                "TrialComponentName": "tc-alpha",
                "DisplayName": "AlphaDisplayed",
                "Status": { "PrimaryStatus": "InProgress", "Message": "running" },
                "StartTime": 1_700_000_000.0,
                "EndTime": 1_700_000_600.0,
            }),
        ),
        (
            "parameters-both-union-arms",
            json!({
                "TrialComponentName": "tc-delta",
                "Status": { "PrimaryStatus": "Completed" },
                "Parameters": {
                    "learning-rate": { "NumberValue": 0.01 },
                    "optimizer": { "StringValue": "adam" },
                },
            }),
        ),
        (
            "artifacts",
            json!({
                "TrialComponentName": "tc-beta",
                "InputArtifacts": {
                    "training-data": { "Value": "s3://bucket/train.csv", "MediaType": "text/csv" },
                },
                "OutputArtifacts": { "model": { "Value": "s3://bucket/model.tar.gz" } },
            }),
        ),
    ]
}

/// A numeric member of a response body.
fn number(body: &Value, member: &str) -> f64 {
    body.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a numeric {member} in {body}"))
}

/// The `TrialComponentName`s a `ListTrialComponents` response lists, in response order.
fn listed_names(body: &Value) -> Vec<String> {
    body.get("TrialComponentSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a TrialComponentSummaries list in {body}"))
        .iter()
        .map(|summary| str_field(summary, "TrialComponentName"))
        .collect()
}

/// Create [`seed_bodies`] in `us-east-1`, spaced so the `CreationTime` order of the result
/// set is unambiguous, and read each one back so the assertions compare against stored
/// state rather than against the request.
async fn seed(emulator: &Emulator) -> Vec<Seeded> {
    let mut seeded = Vec::new();
    for (case, body) in seed_bodies() {
        let created = emulator.ok("CreateTrialComponent", &body).await;
        let name = str_field(&body, "TrialComponentName");
        let described = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": name }),
            )
            .await;
        seeded.push(Seeded {
            case,
            name,
            arn: str_field(&created, "TrialComponentArn"),
            created: number(&described, "CreationTime"),
        });
        tokio::time::sleep(CREATE_SPACING).await;
    }
    seeded
}

/// The names of the seeded components named by `cases`, in that order.
fn names_of(seeded: &[Seeded], cases: &[&str]) -> Vec<String> {
    cases
        .iter()
        .map(|wanted| {
            seeded
                .iter()
                .find(|entry| entry.case == *wanted)
                .unwrap_or_else(|| panic!("no seeded trial component for case {wanted}"))
                .name
                .clone()
        })
        .collect()
}

/// Record that the component `component` is part of the trial `trial`, through
/// `AssociateTrialComponent` — the one call that attaches a component to a trial — so this
/// listing reads the memberships the API writes rather than state planted beside it.
async fn attach_to_trial(emulator: &Emulator, trial: &str, component: &str) {
    emulator
        .ok(
            "AssociateTrialComponent",
            &json!({ "TrialName": trial, "TrialComponentName": component }),
        )
        .await;
}

/// RULE: `ListTrialComponents` returns the account's stored trial components ordered by
/// `SortBy`/`SortOrder` — defaulting to `CreationTime` / `Descending` as both members'
/// model docs state — with `TrialComponentName` as the tie-break.
#[tokio::test]
async fn list_trial_components_honours_every_documented_sort() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator).await;

    let oldest_first = [
        "bare",
        "status-and-times",
        "parameters-both-union-arms",
        "artifacts",
    ];
    let newest_first = {
        let mut reversed = oldest_first;
        reversed.reverse();
        reversed
    };
    // The seeded names, sorted: tc-alpha, tc-beta, tc-delta, tc-gamma.
    let by_name = [
        "status-and-times",
        "artifacts",
        "parameters-both-union-arms",
        "bare",
    ];
    let by_name_descending = {
        let mut reversed = by_name;
        reversed.reverse();
        reversed
    };

    // Rows differ along which sort members the request carries, so each documented default
    // is exercised on its own as well as with the other member set explicitly.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("documented-defaults", json!({}), newest_first.to_vec()),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            oldest_first.to_vec(),
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            newest_first.to_vec(),
        ),
        (
            "name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            by_name.to_vec(),
        ),
        (
            "name-descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            by_name_descending.to_vec(),
        ),
        (
            "sort-by-alone-keeps-the-default-sort-order",
            json!({ "SortBy": "Name" }),
            by_name_descending.to_vec(),
        ),
        (
            "sort-order-alone-keeps-the-default-sort-by",
            json!({ "SortOrder": "Ascending" }),
            oldest_first.to_vec(),
        ),
    ];

    for (case, body, expected) in &cases {
        let listed = emulator.ok("ListTrialComponents", body).await;
        assert_eq!(
            listed_names(&listed),
            names_of(&seeded, expected),
            "case {case}: ListTrialComponents order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page must carry no NextToken, got {listed:?}"
        );
    }

    // The whole modelled grid is accepted and each cell answers the full result set in the
    // order its two members name. The catalogs are the generated
    // `SORT_TRIAL_COMPONENTS_BY_VALUES` / `SORT_ORDER_VALUES`, i.e. the modelled
    // `SortTrialComponentsBy` and `SortOrder` members.
    for sort_by in SORT_TRIAL_COMPONENTS_BY_VALUES {
        for sort_order in SORT_ORDER_VALUES {
            let case = format!("SortBy={sort_by}/SortOrder={sort_order}");
            let listed = emulator
                .ok(
                    "ListTrialComponents",
                    &json!({ "SortBy": sort_by, "SortOrder": sort_order }),
                )
                .await;
            let ascending = *sort_order == "Ascending";
            let expected = match (*sort_by, ascending) {
                ("Name", true) => by_name,
                ("Name", false) => by_name_descending,
                (_, true) => oldest_first,
                (_, false) => newest_first,
            };
            assert_eq!(
                listed_names(&listed),
                names_of(&seeded, &expected),
                "case {case}: ListTrialComponents order, got {listed:?}"
            );
        }
    }
}

/// RULE: every `TrialComponentSummary` replays the stored component — the ARN the create
/// minted, the display name the create resolved, the status and the instants it stored, and
/// the identity it recorded — agreeing member for member with `DescribeTrialComponent`, and
/// a listing changes nothing a later caller sees.
#[tokio::test]
async fn list_trial_component_summaries_replay_the_stored_component() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator).await;

    let listed = emulator
        .ok(
            "ListTrialComponents",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    let summaries = listed
        .get("TrialComponentSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a TrialComponentSummaries list in {listed}"))
        .clone();
    assert_eq!(
        summaries.len(),
        seeded.len(),
        "every stored trial component must be listed, got {listed:?}"
    );

    for (index, entry) in seeded.iter().enumerate() {
        let case = entry.case;
        let summary = &summaries[index];
        assert_eq!(
            str_field(summary, "TrialComponentArn"),
            entry.arn,
            "case {case}: TrialComponentArn in the summary, got {summary:?}"
        );
        // Every member the summary and the reader share reports the same stored value, so
        // the listing is the store rather than an echo of the listing request.
        let described = emulator
            .ok(
                "DescribeTrialComponent",
                &json!({ "TrialComponentName": entry.name }),
            )
            .await;
        for member in [
            "TrialComponentName",
            "TrialComponentArn",
            "DisplayName",
            "Status",
            "StartTime",
            "EndTime",
            "CreationTime",
            "LastModifiedTime",
            "CreatedBy",
            "LastModifiedBy",
        ] {
            assert_eq!(
                summary.get(member),
                described.get(member),
                "case {case}: {member} must agree with DescribeTrialComponent, got {summary:?}"
            );
        }
        // A component created through this API has no source job, so no summary carries a
        // `TrialComponentSource` — the member the reader also leaves absent.
        assert!(
            summary.get("TrialComponentSource").is_none(),
            "case {case}: a component with no source job carries no TrialComponentSource, \
             got {summary:?}"
        );
    }

    // A read changes nothing a later caller sees: the second listing is identical.
    let again = emulator
        .ok(
            "ListTrialComponents",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        again, listed,
        "ListTrialComponents must be stable across reads, got {again:?}"
    );
}

/// RULE: `CreatedAfter` and `CreatedBefore` keep exactly the components on their side of
/// the instant, inclusive of it, and they compose with each other. The bounds are placed
/// between neighbouring instants, which is the finest split the `awsJson1_1` epoch-seconds
/// `Timestamp` represents without loss.
#[tokio::test]
async fn list_trial_components_filters_on_the_created_time_bounds() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator).await;
    let split = (seeded[1].created + seeded[2].created) / 2.0;
    let upper_split = (seeded[2].created + seeded[3].created) / 2.0;

    // Rows differ along which bound is used, which side it selects, how many bounds
    // compose, and whether a bound falls on or outside the whole result set.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "created-before-the-split",
            json!({ "CreatedBefore": split, "SortOrder": "Ascending" }),
            vec!["bare", "status-and-times"],
        ),
        (
            "created-after-the-split",
            json!({ "CreatedAfter": split, "SortOrder": "Ascending" }),
            vec!["parameters-both-union-arms", "artifacts"],
        ),
        (
            "both-bounds-around-one-component",
            json!({
                "CreatedAfter": split,
                "CreatedBefore": upper_split,
                "SortOrder": "Ascending",
            }),
            vec!["parameters-both-union-arms"],
        ),
        (
            "bound-just-before-the-oldest-instant-selects-nothing",
            json!({ "CreatedBefore": seeded[0].created - 0.001 }),
            vec![],
        ),
        (
            "bound-an-hour-past-the-newest-instant-selects-nothing",
            json!({ "CreatedAfter": seeded[3].created + 3600.0 }),
            vec![],
        ),
        (
            "bound-composed-with-a-name-sort",
            json!({
                "CreatedBefore": upper_split,
                "SortBy": "Name",
                "SortOrder": "Ascending",
            }),
            vec!["status-and-times", "parameters-both-union-arms", "bare"],
        ),
    ];

    for (case, body, expected) in &cases {
        let listed = emulator.ok("ListTrialComponents", body).await;
        assert_eq!(
            listed_names(&listed),
            names_of(&seeded, expected),
            "case {case}: ListTrialComponents selection, got {listed:?}"
        );
    }
}

/// Pin the stored creation instant of the component `name` to `at`, on the ONE shared
/// store the handler reads through `state::service_state`.
///
/// The emulator's clock picks the nanoseconds of a create, and the `awsJson1_1`
/// epoch-seconds `Timestamp` an f64 carries cannot name a nanosecond instant without
/// rounding — so a bound placed ON a stored instant is asserted against an instant the
/// wire represents exactly (a whole second), by pinning the stored one to it.
fn pin_creation_instant(emulator: &Emulator, name: &str, at: SystemTime) {
    let state = emulator.engine.state::<ComAmazonawsSagemakerSagemakerState>();
    let mut store = state.lock();
    let component = store
        .trial_components
        .get_mut(&entity_key(name))
        .unwrap_or_else(|| panic!("no stored trial component named {name}"));
    component.creation_time = at;
}

/// RULE: both creation bounds are INCLUSIVE of the instant they name, so a bound equal to
/// a stored `CreationTime` still selects the component carrying it — on either side, and
/// with both bounds on one instant selecting exactly that component.
#[tokio::test]
async fn list_trial_components_creation_bounds_include_the_stored_instant() {
    let emulator = Emulator::new();
    // Rows a minute apart on whole seconds, so every bound below is a value the wire's
    // epoch-seconds `Timestamp` represents exactly rather than a rounded neighbour.
    const BASE_SECOND: u64 = 1_700_000_000;
    let instants = [BASE_SECOND, BASE_SECOND + 60, BASE_SECOND + 120];
    let names = ["inc-one", "inc-two", "inc-three"];
    for (name, second) in names.iter().zip(instants) {
        emulator
            .ok(
                "CreateTrialComponent",
                &json!({ "TrialComponentName": name }),
            )
            .await;
        pin_creation_instant(
            &emulator,
            name,
            SystemTime::UNIX_EPOCH + Duration::from_secs(second),
        );
    }

    // The pinned instants are what the listing reports, so the bounds below are placed on
    // instants a client really reads back.
    let listed = emulator
        .ok(
            "ListTrialComponents",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    let summaries = listed
        .get("TrialComponentSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a TrialComponentSummaries list in {listed}"));
    for (index, second) in instants.iter().enumerate() {
        let case = names[index];
        assert_eq!(
            number(&summaries[index], "CreationTime"),
            *second as f64,
            "case {case}: the reported CreationTime must be the stored instant, got {:?}",
            summaries[index]
        );
    }

    // Rows differ along which bound names the instant, which instant it names, and whether
    // both bounds name the same one.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "created-after-on-the-middle-instant",
            json!({ "CreatedAfter": instants[1] as f64, "SortOrder": "Ascending" }),
            vec!["inc-two", "inc-three"],
        ),
        (
            "created-before-on-the-middle-instant",
            json!({ "CreatedBefore": instants[1] as f64, "SortOrder": "Ascending" }),
            vec!["inc-one", "inc-two"],
        ),
        (
            "both-bounds-on-one-instant",
            json!({
                "CreatedAfter": instants[1] as f64,
                "CreatedBefore": instants[1] as f64,
            }),
            vec!["inc-two"],
        ),
        (
            "created-after-on-the-newest-instant",
            json!({ "CreatedAfter": instants[2] as f64 }),
            vec!["inc-three"],
        ),
        (
            "created-before-on-the-oldest-instant",
            json!({ "CreatedBefore": instants[0] as f64 }),
            vec!["inc-one"],
        ),
        (
            "created-after-one-second-past-the-newest-instant",
            json!({ "CreatedAfter": (instants[2] + 1) as f64 }),
            vec![],
        ),
    ];

    for (case, body, expected) in &cases {
        let listed = emulator.ok("ListTrialComponents", body).await;
        assert_eq!(
            listed_names(&listed),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case}: ListTrialComponents selection, got {listed:?}"
        );
    }
}

/// RULE: `TrialName` keeps the components that are part of that trial, `ExperimentName` the
/// components part of ANY trial of that experiment (each exactly once, however many of its
/// trials hold them), and `SourceArn` the components whose source JOB carries that ARN —
/// of which a component created through `CreateTrialComponent` has none. The names fold
/// case the way the stores key them, and a STORED entity holding nothing selects nothing.
/// A name no entity answers to is refused instead (`list_trial_components_entity_filters`
/// pins that half).
#[tokio::test]
async fn list_trial_components_selects_the_components_of_the_named_trial_or_experiment() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator).await;

    for experiment in ["exp-one", "exp-two"] {
        emulator
            .ok("CreateExperiment", &json!({ "ExperimentName": experiment }))
            .await;
    }
    // Two trials of one experiment and one of another, plus a trial nothing is attached to,
    // so a filter that selects a union, a single trial and an empty trial are all covered.
    for (trial, experiment) in [
        ("trial-a", "exp-one"),
        ("trial-b", "exp-one"),
        ("trial-c", "exp-two"),
        ("trial-empty", "exp-two"),
    ] {
        emulator
            .ok(
                "CreateTrial",
                &json!({ "TrialName": trial, "ExperimentName": experiment }),
            )
            .await;
    }
    // `tc-delta` is part of BOTH trials of `exp-one`, so the experiment filter has to
    // report it once rather than once per trial; `tc-beta` is part of no trial at all.
    attach_to_trial(&emulator, "trial-a", "tc-gamma").await;
    attach_to_trial(&emulator, "trial-a", "tc-delta").await;
    attach_to_trial(&emulator, "trial-b", "tc-delta").await;
    attach_to_trial(&emulator, "trial-b", "tc-alpha").await;
    attach_to_trial(&emulator, "trial-c", "tc-beta").await;

    // Rows differ along which entity filter is sent, how many components it admits, the
    // case the name is written in, whether the named entity exists, and whether the filter
    // composes with a sort or a creation bound.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "trial-of-two-components",
            json!({ "TrialName": "trial-a", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["parameters-both-union-arms", "bare"],
        ),
        (
            "trial-of-one-component",
            json!({ "TrialName": "trial-c" }),
            vec!["artifacts"],
        ),
        (
            // `TrialName` is not case-sensitive, so another spelling names the same trial.
            "trial-name-folds-case",
            json!({ "TrialName": "TRIAL-C" }),
            vec!["artifacts"],
        ),
        (
            "trial-holding-no-component",
            json!({ "TrialName": "trial-empty" }),
            vec![],
        ),
        (
            "experiment-unions-its-trials-without-repeating-a-component",
            json!({ "ExperimentName": "exp-one", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["status-and-times", "parameters-both-union-arms", "bare"],
        ),
        (
            "experiment-of-one-trial",
            json!({ "ExperimentName": "exp-two" }),
            vec!["artifacts"],
        ),
        (
            "experiment-name-folds-case",
            json!({ "ExperimentName": "EXP-TWO" }),
            vec!["artifacts"],
        ),
        (
            // No stored component was produced by a job, so none carries a source ARN.
            "source-arn-of-a-job",
            json!({ "SourceArn": format!("arn:aws:sagemaker:us-east-1:000000000000:training-job/{}", "tj-one") }),
            vec![],
        ),
        (
            "source-arn-empty-string",
            json!({ "SourceArn": "" }),
            vec![],
        ),
        (
            "trial-filter-composed-with-a-creation-bound",
            json!({
                "TrialName": "trial-a",
                "CreatedBefore": (seeded[0].created + seeded[1].created) / 2.0,
            }),
            vec!["bare"],
        ),
        (
            "no-entity-filter-lists-the-whole-account",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            vec![
                "status-and-times",
                "artifacts",
                "parameters-both-union-arms",
                "bare",
            ],
        ),
    ];

    for (case, body, expected) in &cases {
        let listed = emulator.ok("ListTrialComponents", body).await;
        assert_eq!(
            listed_names(&listed),
            names_of(&seeded, expected),
            "case {case}: ListTrialComponents selection, got {listed:?}"
        );
    }

    // An entity filter pages like any other result set: the trial's two components come
    // back one per page and the walk ends there.
    let first = emulator
        .ok(
            "ListTrialComponents",
            &json!({
                "TrialName": "trial-a",
                "MaxResults": 1,
                "SortBy": "Name",
                "SortOrder": "Ascending",
            }),
        )
        .await;
    let token = str_field(&first, "NextToken");
    let second = emulator
        .ok(
            "ListTrialComponents",
            &json!({
                "TrialName": "trial-a",
                "MaxResults": 1,
                "SortBy": "Name",
                "SortOrder": "Ascending",
                "NextToken": token,
            }),
        )
        .await;
    assert_eq!(
        [listed_names(&first), listed_names(&second)].concat(),
        names_of(&seeded, &["parameters-both-union-arms", "bare"]),
        "case trial-filter-pages: the walk must cover the trial's components exactly once, \
         got {first:?} then {second:?}"
    );
    assert!(
        second.get("NextToken").is_none(),
        "case trial-filter-pages: the last page must carry no NextToken, got {second:?}"
    );
}

/// RULE: paging walks the whole ordered result exactly once — each page holds at most the
/// effective page size, a `NextToken` is present only while more remain, and feeding it
/// back resumes immediately after the previous page. The effective size is `MaxResults`
/// where the caller sends one (capped by the service's own ceiling) and this operation's
/// documented default of 10 where it does not.
#[tokio::test]
async fn list_trial_components_paging_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    // More components than the documented default page, so a defaulted page and an
    // explicitly sized one are distinguishable.
    let mut names: Vec<String> = Vec::new();
    for index in 0..12 {
        let name = format!("page-{index:02}");
        emulator
            .ok(
                "CreateTrialComponent",
                &json!({ "TrialComponentName": name }),
            )
            .await;
        names.push(name);
        tokio::time::sleep(CREATE_SPACING).await;
    }

    // Rows: every page size from one entry up to past the end of the result set, plus both
    // published bounds of `MaxResults` (`range 1..100`, from
    // smithy_query(query="operation",operation="ListTrialComponents")).
    for page_size in [1_usize, 2, 5, 11, 12, 13, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "MaxResults": page_size, "SortOrder": "Ascending" });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListTrialComponents", &body).await;
            let page = listed_names(&listed);
            let cap = page_size.min(PAGE_CEILING);
            assert!(
                page.len() <= cap,
                "case {case}: a page must hold at most {cap} entries, got {page:?}"
            );
            seen.extend(page);
            token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case}: paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case {case}: paging must walk the whole ordered result exactly once"
        );
    }

    // No `MaxResults`: the page is this operation's documented default of 10, and the token
    // still walks the rest.
    let first = emulator
        .ok("ListTrialComponents", &json!({ "SortOrder": "Ascending" }))
        .await;
    assert_eq!(
        listed_names(&first).len(),
        DOCUMENTED_DEFAULT_MAX_RESULTS,
        "case documented-default: the page size must default to 10, got {first:?}"
    );
    let token = str_field(&first, "NextToken");
    let second = emulator
        .ok(
            "ListTrialComponents",
            &json!({ "NextToken": token, "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        listed_names(&second),
        names[DOCUMENTED_DEFAULT_MAX_RESULTS..].to_vec(),
        "case documented-default: the token must resume after the first page, got {second:?}"
    );
    assert!(
        second.get("NextToken").is_none(),
        "case documented-default: the last page must carry no NextToken, got {second:?}"
    );
}

/// RULE: the page is capped server-side, so no single call serialises the whole store
/// however large `MaxResults` is — and the resume token still walks the full set.
#[tokio::test]
async fn the_trial_component_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    let emulator = Emulator::new();
    // Seeded past the ceiling so a capped page and an uncapped one are distinguishable.
    // The components are created without spacing, so the walk below compares against the
    // names sorted rather than against creation order.
    let mut names: Vec<String> = Vec::new();
    for index in 0..PAGE_CEILING + 3 {
        let name = format!("cap-{index:03}");
        emulator
            .ok(
                "CreateTrialComponent",
                &json!({ "TrialComponentName": name }),
            )
            .await;
        names.push(name);
    }

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "MaxResults": 100, "SortBy": "Name", "SortOrder": "Ascending" });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = emulator.ok("ListTrialComponents", &body).await;
        let page = listed_names(&listed);
        assert!(
            page.len() <= PAGE_CEILING,
            "case ceiling: a page must hold at most {PAGE_CEILING} entries, got {}",
            page.len()
        );
        seen.extend(page);
        token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    names.sort();
    assert_eq!(
        seen, names,
        "case ceiling: the resume token must walk the whole result exactly once"
    );
}

/// RULE: an input outside a documented value set — and a request naming more than one of
/// the three mutually exclusive entity filters — is refused with HTTP 400
/// `ValidationException` rather than silently ignored, and the refusal changes nothing.
#[tokio::test]
async fn list_trial_components_refuses_inputs_outside_the_documented_value_sets() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator).await;
    // A trial the account really holds, so the stale-token row below is refused for its
    // TOKEN rather than for naming a trial that does not exist.
    emulator
        .ok("CreateExperiment", &json!({ "ExperimentName": "token-exp" }))
        .await;
    emulator
        .ok(
            "CreateTrial",
            &json!({ "TrialName": "token-trial", "ExperimentName": "token-exp" }),
        )
        .await;

    let token_of_a_longer_result_set = {
        let page = emulator
            .ok(
                "ListTrialComponents",
                &json!({ "MaxResults": 1, "SortBy": "Name", "SortOrder": "Ascending" }),
            )
            .await;
        str_field(&page, "NextToken")
    };
    // Rows: one outside each documented value set — `SortTrialComponentsBy`, `SortOrder`,
    // `MaxResults` one past each published bound (`range 1..100`), `TrialName` outside
    // `ExperimentEntityName`'s `len 1..120` and its pattern, `SourceArn` past `String256`'s
    // 256, `NextToken` past 8192, a token that names no boundary of the result set, and
    // each pair (and the triple) of the mutually exclusive entity filters.
    let cases: Vec<(&str, Value)> = vec![
        ("sort-by-outside-the-enum", json!({ "SortBy": "Status" })),
        (
            "sort-order-outside-the-enum",
            json!({ "SortOrder": "Sideways" }),
        ),
        ("max-results-below-1", json!({ "MaxResults": 0 })),
        ("max-results-above-100", json!({ "MaxResults": 101 })),
        ("trial-name-empty", json!({ "TrialName": "" })),
        (
            "trial-name-one-past-the-maximum",
            json!({ "TrialName": "t".repeat(121) }),
        ),
        (
            "experiment-name-outside-the-pattern",
            json!({ "ExperimentName": "-leading-hyphen" }),
        ),
        (
            "source-arn-one-past-the-maximum",
            json!({ "SourceArn": "s".repeat(257) }),
        ),
        (
            "next-token-one-past-the-maximum",
            json!({ "NextToken": "a".repeat(8193) }),
        ),
        (
            "next-token-that-names-no-boundary",
            json!({ "NextToken": "not-a-token" }),
        ),
        (
            // A token minted against the whole result set names a component the filtered
            // one does not hold, so it is refused rather than silently restarting the walk.
            "next-token-of-another-result-set",
            json!({
                "NextToken": token_of_a_longer_result_set,
                "TrialName": "token-trial",
                "SortBy": "Name",
                "SortOrder": "Ascending",
            }),
        ),
        (
            "experiment-name-with-trial-name",
            json!({ "ExperimentName": "exp-one", "TrialName": "trial-a" }),
        ),
        (
            "experiment-name-with-source-arn",
            json!({ "ExperimentName": "exp-one", "SourceArn": "arn:aws:sagemaker:us-east-1:000000000000:training-job/tj" }),
        ),
        (
            "trial-name-with-source-arn",
            json!({ "TrialName": "trial-a", "SourceArn": "arn:aws:sagemaker:us-east-1:000000000000:training-job/tj" }),
        ),
        (
            "all-three-entity-filters",
            json!({
                "ExperimentName": "exp-one",
                "TrialName": "trial-a",
                "SourceArn": "arn:aws:sagemaker:us-east-1:000000000000:training-job/tj",
            }),
        ),
    ];

    for (case, body) in &cases {
        let (status, parsed) = emulator.call("ListTrialComponents", body).await;
        assert_eq!(
            status, 400,
            "case {case}: ListTrialComponents must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: refusal code, got {parsed}"
        );
    }

    // Nothing above reached state: every seeded component is still listed, in order.
    let listed = emulator
        .ok(
            "ListTrialComponents",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        listed_names(&listed),
        seeded
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<Vec<String>>(),
        "a refused request must change nothing, got {listed:?}"
    );
}
