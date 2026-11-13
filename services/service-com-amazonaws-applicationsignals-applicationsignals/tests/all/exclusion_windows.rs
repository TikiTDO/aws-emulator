// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the exclusion windows configured on a service level objective:
//! `BatchUpdateExclusionWindows` (PATCH `/exclusion-windows`) and
//! `ListServiceLevelObjectiveExclusionWindows` (GET `/slo/{Id}/exclusion-windows`).
//!
//! The rule under test: a batch update applies its removals and then its additions to the
//! stored window set of every SLO its `SloIds` names, independently per id — reporting the
//! ids it applied them to and one `Errors` entry per id it did not — and a later read of an
//! SLO reports exactly the configurations its own set holds, in the order they were added.
//!
//! Every test drives the real `restJson1` surface through the router that production mounts,
//! and each rule is asserted over a TABLE of window configurations: the recorded shape of one
//! window is one row.
//!
//! The request helpers (`router`, `call`, …) are the ones `slo_lifecycle` already drives this
//! service's surface with, so every suite speaks to one harness.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{
    arn_path_segment, call, create, name_path_segment, router, sli_config, ACCOUNT, REGION,
};

/// Every value of the modelled `DurationUnit` enum
/// (`smithy_query(shape="DurationUnit")` -> `["MINUTE", "HOUR", "DAY", "MONTH"]`), which is
/// `Window.DurationUnit`'s target.
pub(crate) const DURATION_UNITS: [&str; 4] = ["MINUTE", "HOUR", "DAY", "MONTH"];

/// The most exclusion windows one SLO holds: `AddExclusionWindows`'s doc ("You can add up to
/// 10 exclusion windows per SLO") and the modelled `ExclusionWindows` length maximum.
pub(crate) const MAX_WINDOWS_PER_SLO: usize = 10;

/// The `ResourceType` every SLO failure reports.
const SLO_RESOURCE_TYPE: &str = "Slo";

/// One exclusion window, carrying only the members the row states.
///
/// `Window` is the shape's only `@required` member; `StartTime`, `RecurrenceRule` and
/// `Reason` are optional, and a row that omits one sends no such key at all
/// (`smithy_query(shape="ExclusionWindow")`).
pub(crate) fn window(
    duration_unit: &str,
    duration: i64,
    start_time: Option<i64>,
    recurrence: Option<&str>,
    reason: Option<&str>,
) -> Value {
    let mut window = json!({ "Window": { "DurationUnit": duration_unit, "Duration": duration } });
    if let Some(start_time) = start_time {
        window["StartTime"] = json!(start_time);
    }
    if let Some(expression) = recurrence {
        window["RecurrenceRule"] = json!({ "Expression": expression });
    }
    if let Some(reason) = reason {
        window["Reason"] = json!(reason);
    }
    window
}

/// The configuration of one exclusion window, as a value two JSON renderings of the same
/// window compare equal on.
///
/// `StartTime` is an epoch-seconds `Timestamp` on `restJson1`, so a request may spell it as
/// an integer where the response renders it as a float; it is compared as a number to three
/// decimal places rather than as JSON, which would make `1` and `1.0` different windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowKey {
    duration_unit: Option<String>,
    duration: Option<i64>,
    start_time: Option<String>,
    recurrence: Option<String>,
    reason: Option<String>,
}

fn key_of(window: &Value) -> WindowKey {
    WindowKey {
        duration_unit: window["Window"]["DurationUnit"]
            .as_str()
            .map(ToString::to_string),
        duration: window["Window"]["Duration"].as_i64(),
        start_time: window["StartTime"]
            .as_f64()
            .map(|seconds| format!("{seconds:.3}")),
        recurrence: window["RecurrenceRule"]["Expression"]
            .as_str()
            .map(ToString::to_string),
        reason: window["Reason"].as_str().map(ToString::to_string),
    }
}

/// The configurations a list page reports, in order.
pub(crate) fn keys_of(page: &Value) -> Vec<WindowKey> {
    page["ExclusionWindows"]
        .as_array()
        .map(|windows| windows.iter().map(key_of).collect())
        .unwrap_or_default()
}

/// The configurations `windows` states, in order.
pub(crate) fn keys_sent(windows: &[Value]) -> Vec<WindowKey> {
    windows.iter().map(key_of).collect()
}

/// The windows a list page reports, as the JSON configurations a later call can name them by.
pub(crate) fn windows_of(page: &Value) -> Vec<Value> {
    page["ExclusionWindows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Assert that `reported` holds exactly the configurations `sent`, in order, in the form the
/// service STORES them: every member as sent, and — where a row omitted `StartTime` — some
/// concrete instant, since that member "defaults to current time if not specified"
/// (`smithy_query(shape="ExclusionWindow")`, member `StartTime`). WHICH instant the service
/// settles an omission to is pinned by the `exclusion_window_start_time` suite; here it is
/// only required to be a real instant, so a suite about some other rule does not depend on
/// the clock.
pub(crate) fn assert_stored(case: &str, reported: &[WindowKey], sent: &[Value]) {
    let expected = keys_sent(sent);
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {case}: expected {} stored window(s), got {reported:?}",
        expected.len()
    );
    for (index, (actual, wanted)) in reported.iter().zip(expected.iter()).enumerate() {
        match wanted.start_time.as_ref() {
            Some(stated) => assert_eq!(
                actual.start_time.as_ref(),
                Some(stated),
                "case {case} window {index}: expected the StartTime sent ({stated}), got {actual:?}"
            ),
            None => assert!(
                actual.start_time.is_some(),
                "case {case} window {index}: expected a defaulted StartTime, got {actual:?}"
            ),
        }
        assert_eq!(
            WindowKey {
                start_time: None,
                ..actual.clone()
            },
            WindowKey {
                start_time: None,
                ..wanted.clone()
            },
            "case {case} window {index}: expected the configuration sent, got {actual:?}"
        );
    }
}

/// One `BatchUpdateExclusionWindows` call. A list the row leaves empty is omitted from the
/// body, since both are optional members.
pub(crate) async fn batch(
    router: &MultiServiceRouter,
    slo_ids: &[String],
    add: &[Value],
    remove: &[Value],
) -> (u16, Value) {
    let mut body = json!({ "SloIds": slo_ids });
    if !add.is_empty() {
        body["AddExclusionWindows"] = json!(add);
    }
    if !remove.is_empty() {
        body["RemoveExclusionWindows"] = json!(remove);
    }
    call(router, "PATCH", "/exclusion-windows", Some(body)).await
}

/// One `ListServiceLevelObjectiveExclusionWindows` call on the SLO called `name`, with the
/// query string `query` (`""` for none).
pub(crate) async fn list(router: &MultiServiceRouter, name: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!(
            "/slo/{}/exclusion-windows{query}",
            name_path_segment(name)
        ),
        None,
    )
    .await
}

/// The query string of a read bounded by `max_results` and resuming from `token`, each
/// omitted when the caller states none.
///
/// The ONE place this test target spells this read's query string, so two suites cannot send
/// a page request differently.
pub(crate) fn query(max_results: Option<usize>, token: Option<&str>) -> String {
    match (max_results, token) {
        (None, None) => String::new(),
        (Some(size), None) => format!("?MaxResults={size}"),
        (None, Some(token)) => format!("?NextToken={token}"),
        (Some(size), Some(token)) => format!("?MaxResults={size}&NextToken={token}"),
    }
}

/// Every window the SLO called `name` reports when the caller follows `NextToken` from the
/// first page at a bound of `max_results`, and how many pages that took.
///
/// Each page must succeed and must hold at most the bound the caller stated; paging that does
/// not terminate within one page per window the SLO can hold fails here rather than looping.
pub(crate) async fn paged(
    router: &MultiServiceRouter,
    name: &str,
    max_results: Option<usize>,
) -> (Vec<WindowKey>, usize) {
    let mut collected: Vec<WindowKey> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let (status, page) = list(router, name, &query(max_results, token.as_deref())).await;
        assert_eq!(
            status, 200,
            "case {name:?}/{max_results:?}/page {pages}: expected the read to succeed, got {status} with {page}"
        );
        let keys = keys_of(&page);
        if let Some(size) = max_results {
            assert!(
                keys.len() <= size,
                "case {name:?}/{max_results:?}/page {pages}: expected at most {size} windows, got {}",
                keys.len()
            );
        }
        collected.extend(keys);
        token = page["NextToken"].as_str().map(ToString::to_string);
        pages += 1;
        if token.is_none() {
            return (collected, pages);
        }
        assert!(
            pages <= MAX_WINDOWS_PER_SLO + 1,
            "case {name:?}/{max_results:?}: paging did not terminate after {pages} pages"
        );
    }
}

/// The ARN of the SLO called `name` in this account and region.
pub(crate) fn slo_arn(name: &str) -> String {
    format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/{name}")
}

/// Create the SLO called `name`, failing the test if the setup create does not succeed.
pub(crate) async fn create_slo(router: &MultiServiceRouter, name: &str) {
    let (status, created) = create(
        router,
        json!({ "Name": name, "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected the setup create to succeed, got {status} with {created}"
    );
}

/// The `SloIds` a batch response reports, in order.
pub(crate) fn applied_ids(response: &Value) -> Vec<String> {
    response["SloIds"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A table of exclusion-window configurations spanning the class: every modelled
/// `DurationUnit`, the `Duration` range minimum of 1 and larger counts, a stated and an
/// omitted `StartTime`, a cron and a rate `RecurrenceRule` and none at all, and a stated and
/// an omitted `Reason`.
pub(crate) fn window_table() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minute, duration 1, no optional member",
            window(DURATION_UNITS[0], 1, None, None, None),
        ),
        (
            "hour, dated, with a reason",
            window(
                DURATION_UNITS[1],
                6,
                Some(1_762_000_000),
                None,
                Some("Scheduled database migration"),
            ),
        ),
        (
            "day, dated, cron recurrence, with a reason",
            window(
                DURATION_UNITS[2],
                2,
                Some(1_700_000_000),
                Some("cron(0 0 ? * SUN *)"),
                Some("Weekly maintenance"),
            ),
        ),
        (
            "month, rate recurrence, no start time",
            window(DURATION_UNITS[3], 1, None, Some("rate(30 days)"), None),
        ),
        (
            "day, dated, no recurrence, reason at the length maximum",
            window(
                DURATION_UNITS[2],
                365,
                Some(0),
                None,
                // `ExclusionReason` is `length 1..1024`
                // (`smithy_query(shape="ExclusionWindow")`, member `Reason`).
                Some(&"r".repeat(1024)),
            ),
        ),
        (
            "minute, dated, rate recurrence, reason at the length minimum",
            window(
                DURATION_UNITS[0],
                45,
                Some(1_762_000_000),
                Some("rate(1 hour)"),
                Some("x"),
            ),
        ),
    ]
}

/// Every configuration the table spans is stored as sent — bar the members the service
/// defaults — reported back by a later read, and removed again by the configuration that read
/// reports.
///
/// The rows differ along what the rule generalises over: the duration unit, the duration, and
/// which of the three optional members the configuration states.
#[tokio::test]
async fn each_window_configuration_is_stored_reported_and_removed_by_its_configuration() {
    let router = router();
    for (index, (label, sent)) in window_table().into_iter().enumerate() {
        let name = format!("exclusion-row-{index}");
        create_slo(&router, &name).await;

        let (status, updated) = batch(&router, std::slice::from_ref(&name), std::slice::from_ref(&sent), &[]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the add to succeed, got {status} with {updated}"
        );
        assert_eq!(
            applied_ids(&updated),
            vec![name.clone()],
            "case {label:?}: expected SloIds to echo the id the window was applied to, got {updated}"
        );
        assert_eq!(
            updated["Errors"].as_array().map(Vec::len),
            Some(0),
            "case {label:?}: expected no per-SLO errors, got {updated}"
        );

        let (status, page) = list(&router, &name, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the read to succeed, got {status} with {page}"
        );
        assert_stored(label, &keys_of(&page), std::slice::from_ref(&sent));

        // A removal names a window by the configuration the read reports for it, which is the
        // configuration sent with every server-defaulted member settled.
        let stored = windows_of(&page);
        let (status, removed) = batch(&router, std::slice::from_ref(&name), &[], &stored).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the removal to succeed, got {status} with {removed}"
        );
        let (status, page) = list(&router, &name, "").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the read after the removal to succeed, got {status} with {page}"
        );
        assert_eq!(
            keys_of(&page),
            Vec::new(),
            "case {label:?}: expected the window set to be empty after the removal, got {page}"
        );
    }
}

/// One call applies the same window set to every SLO its `SloIds` names, whichever id form
/// the modelled `ServiceLevelObjectiveId` admits each was named by, and a repeated SLO is
/// applied once.
#[tokio::test]
async fn one_call_applies_the_windows_to_every_slo_it_names() {
    let router = router();
    let names = ["batch-by-name", "batch-by-arn", "batch-by-name-again"];
    for name in &names {
        create_slo(&router, name).await;
    }

    let table = window_table();
    let sent: Vec<Value> = table.iter().take(3).map(|(_, value)| value.clone()).collect();
    // The ids differ along the two forms the modelled pattern admits: a bare name and the
    // SLO's full ARN.
    let ids = vec![
        names[0].to_string(),
        slo_arn(names[1]),
        names[2].to_string(),
    ];

    let (status, updated) = batch(&router, &ids, &sent, &[]).await;
    assert_eq!(
        status, 200,
        "case all-ids: expected the batch to succeed, got {status} with {updated}"
    );
    assert_eq!(
        applied_ids(&updated),
        ids,
        "case all-ids: expected SloIds to echo every id as sent, got {updated}"
    );
    for name in &names {
        let (status, page) = list(&router, name, "").await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the read to succeed, got {status} with {page}"
        );
        assert_stored(name, &keys_of(&page), &sent);
    }

    // The same SLO named twice, once by name and once by ARN, is one SLO: it is applied once
    // (so the window set does not gain the addition twice) and reported once.
    let repeated = "batch-repeated-id";
    create_slo(&router, repeated).await;
    let (status, updated) = batch(
        &router,
        &[repeated.to_string(), slo_arn(repeated)],
        &sent,
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case repeated-id: expected the batch to succeed, got {status} with {updated}"
    );
    assert_eq!(
        applied_ids(&updated),
        vec![repeated.to_string()],
        "case repeated-id: expected the repeated SLO to be reported once, got {updated}"
    );
    let (_, page) = list(&router, repeated, "").await;
    assert_stored("repeated-id", &keys_of(&page), &sent);
}

/// A batch reports each id it could not apply the update to in `Errors`, and still applies it
/// to every other id it names.
#[tokio::test]
async fn a_batch_reports_the_ids_it_could_not_apply_and_applies_the_rest() {
    let router = router();
    let stored = ["partial-stored-a", "partial-stored-b"];
    for name in &stored {
        create_slo(&router, name).await;
    }
    // The missing ids differ along the id form, so the per-item error is reported for either.
    let missing = ["partial-absent".to_string(), slo_arn("partial-absent-arn")];
    let ids = vec![
        stored[0].to_string(),
        missing[0].clone(),
        stored[1].to_string(),
        missing[1].clone(),
    ];
    let sent: Vec<Value> = window_table()
        .into_iter()
        .take(2)
        .map(|(_, value)| value)
        .collect();

    let (status, updated) = batch(&router, &ids, &sent, &[]).await;
    assert_eq!(
        status, 200,
        "case partial: expected HTTP 200 for a batch that applied to some ids, got {status} with {updated}"
    );
    assert_eq!(
        applied_ids(&updated),
        vec![stored[0].to_string(), stored[1].to_string()],
        "case partial: expected SloIds to hold only the ids that were applied, got {updated}"
    );

    let errors = updated["Errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        missing.len(),
        "case partial: expected one error per id that was not applied, got {updated}"
    );
    for (error, id) in errors.iter().zip(missing.iter()) {
        let expected_arn = if id.starts_with("arn:") {
            id.clone()
        } else {
            slo_arn(id)
        };
        assert_eq!(
            error["SloId"].as_str(),
            Some(id.as_str()),
            "case {id:?}: expected the error to name the id as sent, got {error}"
        );
        assert_eq!(
            error["ErrorCode"].as_str(),
            Some("ResourceNotFoundException"),
            "case {id:?}: expected the modelled exception's name as the code, got {error}"
        );
        assert_eq!(
            error["ErrorMessage"].as_str(),
            Some(format!("{SLO_RESOURCE_TYPE} with arn '{expected_arn}' does not exist.").as_str()),
            "case {id:?}: expected the message to identify the SLO by its ARN, got {error}"
        );
    }

    for name in &stored {
        let (_, page) = list(&router, name, "").await;
        assert_stored(name, &keys_of(&page), &sent);
    }
}

/// A batch that could apply to NO id it names is refused with the modelled exception that
/// id's failure earns on its own, and stores nothing.
#[tokio::test]
async fn a_batch_that_applies_to_nothing_is_refused_with_the_modelled_exception() {
    let router = router();
    let stored = "refusal-stored";
    create_slo(&router, stored).await;
    let sent: Vec<Value> = window_table()
        .into_iter()
        .take(1)
        .map(|(_, value)| value)
        .collect();

    // Every id names no SLO: the ARN form and the bare-name form, alone and together.
    for ids in [
        vec!["refusal-absent".to_string()],
        vec![slo_arn("refusal-absent")],
        vec!["refusal-absent-one".to_string(), slo_arn("refusal-absent-two")],
    ] {
        let (status, error) = batch(&router, &ids, &sent, &[]).await;
        assert_eq!(
            status, 404,
            "case {ids:?}: expected HTTP 404 when no id names a stored SLO, got {status} with {error}"
        );
        let expected_arn = if ids[0].starts_with("arn:") {
            ids[0].clone()
        } else {
            slo_arn(&ids[0])
        };
        assert_eq!(
            error["ResourceType"].as_str(),
            Some(SLO_RESOURCE_TYPE),
            "case {ids:?}: expected ResourceType {SLO_RESOURCE_TYPE:?}, got {error}"
        );
        assert_eq!(
            error["ResourceId"].as_str(),
            Some(expected_arn.as_str()),
            "case {ids:?}: expected ResourceId {expected_arn:?}, got {error}"
        );
    }

    // A removal that matches no stored window is a successful no-op rather than a refusal of
    // the batch; that rule, and the set it leaves behind, is pinned by the
    // `exclusion_window_removal` suite.
}

/// An SLO holds at most ten exclusion windows: an addition that would take it past the limit
/// is refused and changes nothing, while an addition of a configuration it already holds
/// consumes no slot.
#[tokio::test]
async fn an_slo_holds_at_most_ten_exclusion_windows() {
    let router = router();
    let name = "capped-slo";
    create_slo(&router, name).await;

    // Ten distinct configurations, differing in duration, built over every modelled duration
    // unit: `AddExclusionWindows` is itself `len ..10`, so this is the largest single call.
    let full: Vec<Value> = (0..MAX_WINDOWS_PER_SLO)
        .map(|index| {
            window(
                DURATION_UNITS[index % DURATION_UNITS.len()],
                index as i64 + 1,
                None,
                None,
                None,
            )
        })
        .collect();
    let (status, updated) = batch(&router, &[name.to_string()], &full, &[]).await;
    assert_eq!(
        status, 200,
        "case fill: expected ten windows to be accepted, got {status} with {updated}"
    );
    let (_, page) = list(&router, name, "").await;
    assert_stored("fill", &keys_of(&page), &full);
    // The ten windows as the service stores them: the configurations sent, each with its
    // `StartTime` settled, which is how a later call names a window it already holds.
    let stored = windows_of(&page);

    // Re-adding configurations the SLO already holds is a no-op: the set is identified by
    // configuration, so no slot is consumed and the call succeeds.
    let (status, updated) = batch(&router, &[name.to_string()], &stored[..3], &[]).await;
    assert_eq!(
        status, 200,
        "case repeat: expected re-adding stored configurations to succeed, got {status} with {updated}"
    );
    let (_, page) = list(&router, name, "").await;
    assert_eq!(
        keys_of(&page),
        keys_sent(&stored),
        "case repeat: expected the set to be unchanged, got {page}"
    );

    // One more distinct configuration would make eleven.
    let eleventh = window(DURATION_UNITS[0], 999, None, None, Some("one too many"));
    let (status, error) = batch(
        &router,
        &[name.to_string()],
        std::slice::from_ref(&eleventh),
        &[],
    )
    .await;
    assert_eq!(
        status, 400,
        "case overflow: expected HTTP 400 past the per-SLO limit, got {status} with {error}"
    );
    assert_eq!(
        error["message"].as_str(),
        Some(
            "1 validation error detected: Value at 'addExclusionWindows' failed to satisfy \
             constraint: Member must not take the SLO past 10 exclusion windows"
        ),
        "case overflow: expected the refusal to name the member and the limit, got {error}"
    );
    let (_, page) = list(&router, name, "").await;
    assert_eq!(
        keys_of(&page),
        keys_sent(&stored),
        "case overflow: expected the refused addition to leave the set untouched, got {page}"
    );

    // Removing a window frees its slot, and removals are applied before additions — so one
    // call can replace a window even at the limit.
    let (status, updated) = batch(
        &router,
        &[name.to_string()],
        std::slice::from_ref(&eleventh),
        &stored[..1],
    )
    .await;
    assert_eq!(
        status, 200,
        "case replace: expected a removal plus an addition to succeed at the limit, got {status} with {updated}"
    );
    let mut expected: Vec<Value> = stored[1..].to_vec();
    expected.push(eleventh);
    let (_, page) = list(&router, name, "").await;
    assert_stored("replace", &keys_of(&page), &expected);
}

/// A read pages over the window set: following `NextToken` reports every window exactly once,
/// in the stored order, for every page size the model admits.
#[tokio::test]
async fn a_read_pages_over_the_window_set_for_every_page_size_the_model_admits() {
    let router = router();
    let name = "paged-slo";
    create_slo(&router, name).await;
    let sent: Vec<Value> = window_table().into_iter().map(|(_, value)| value).collect();
    let (status, updated) = batch(&router, &[name.to_string()], &sent, &[]).await;
    assert_eq!(
        status, 200,
        "case setup: expected the setup add to succeed, got {status} with {updated}"
    );

    // `ListServiceLevelObjectiveExclusionWindowsMaxResults` is `range 1..10`
    // (`smithy_query(query="operation",operation=
    // "ListServiceLevelObjectiveExclusionWindows")`): both published bounds and the sizes
    // between, plus the omitted bound, which reports the whole set.
    for max_results in [Some(1), Some(2), Some(3), Some(5), Some(10), None] {
        let (collected, pages) = paged(&router, name, max_results).await;
        assert_stored(&format!("MaxResults {max_results:?}"), &collected, &sent);
        let expected_pages = max_results.map_or(1, |size| sent.len().div_ceil(size).max(1));
        assert_eq!(
            pages, expected_pages,
            "case {max_results:?}: expected {expected_pages} page(s) for {} windows, got {pages}",
            sent.len()
        );
    }

    // A `NextToken` this service did not issue is refused, not honoured as a boundary.
    for forged in ["not-hex", "00", "736c6f732f312f78"] {
        let (status, error) = list(&router, name, &format!("?NextToken={forged}")).await;
        assert_eq!(
            status, 400,
            "case {forged:?}: expected HTTP 400 for a token this service did not issue, got {status} with {error}"
        );
    }

    // A `MaxResults` outside the modelled range is refused one step past either bound.
    for max_results in [0, 11] {
        let (status, error) = list(&router, name, &format!("?MaxResults={max_results}")).await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected HTTP 400 outside the modelled range, got {status} with {error}"
        );
    }
}

/// An `Id` that names no stored SLO is refused by the read, whichever form it takes — holding
/// no windows and not existing are different answers — and an SLO's window set does not
/// outlive the SLO.
#[tokio::test]
async fn a_read_refuses_an_unknown_slo_and_a_deleted_slo_keeps_no_windows() {
    let router = router();
    let name = "deleted-window-slo";
    create_slo(&router, name).await;
    let sent: Vec<Value> = window_table()
        .into_iter()
        .take(2)
        .map(|(_, value)| value)
        .collect();
    let (status, updated) = batch(&router, &[name.to_string()], &sent, &[]).await;
    assert_eq!(
        status, 200,
        "case setup: expected the setup add to succeed, got {status} with {updated}"
    );

    for (label, segment, id) in [
        ("never created, by name", name_path_segment("absent-slo"), "absent-slo".to_string()),
        ("never created, by ARN", arn_path_segment("absent-slo"), slo_arn("absent-slo")),
    ] {
        let (status, error) = call(
            &router,
            "GET",
            &format!("/slo/{segment}/exclusion-windows"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404 for an SLO that is not stored, got {status} with {error}"
        );
        let arn = if id.starts_with("arn:") { id } else { slo_arn(&id) };
        assert_eq!(
            error["ResourceId"].as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected ResourceId {arn:?}, got {error}"
        );
    }

    // The windows belong to the SLO: deleting it drops them, so an SLO created again under
    // the same name is a new resource with none.
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/slo/{}", name_path_segment(name)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete: expected the delete to succeed, got {status} with {deleted}"
    );
    let (status, error) = list(&router, name, "").await;
    assert_eq!(
        status, 404,
        "case delete: expected HTTP 404 after the SLO was deleted, got {status} with {error}"
    );

    create_slo(&router, name).await;
    let (status, page) = list(&router, name, "").await;
    assert_eq!(
        status, 200,
        "case recreate: expected the read to succeed, got {status} with {page}"
    );
    assert_eq!(
        keys_of(&page),
        Vec::new(),
        "case recreate: expected a re-created SLO to hold no exclusion windows, got {page}"
    );
}

/// A batch carrying neither list changes nothing and reports which ids it names are stored.
#[tokio::test]
async fn a_batch_with_no_windows_changes_nothing() {
    let router = router();
    let name = "no-op-slo";
    create_slo(&router, name).await;
    let sent: Vec<Value> = window_table()
        .into_iter()
        .take(2)
        .map(|(_, value)| value)
        .collect();
    let (status, updated) = batch(&router, &[name.to_string()], &sent, &[]).await;
    assert_eq!(
        status, 200,
        "case setup: expected the setup add to succeed, got {status} with {updated}"
    );

    let (status, updated) = batch(&router, &[name.to_string()], &[], &[]).await;
    assert_eq!(
        status, 200,
        "case no-op: expected a batch with neither list to succeed, got {status} with {updated}"
    );
    assert_eq!(
        applied_ids(&updated),
        vec![name.to_string()],
        "case no-op: expected the id to be reported as processed, got {updated}"
    );
    let (_, page) = list(&router, name, "").await;
    assert_stored("no-op", &keys_of(&page), &sent);
}
