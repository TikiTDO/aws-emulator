// SPDX-License-Identifier: CC0-1.0

//! `ListSyncResources` end to end over HTTP.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses), so the route, both `@httpLabel`s, the
//! status code and the serialized body are all exercised, not just a handler
//! body.
//!
//! The suites assert RULES over tables of inputs rather than one recorded case:
//!
//! * a listing reports the resources the sync job the request ADDRESSES holds —
//!   the workspace and the sync source together — in `externalId` order, each row
//!   reporting exactly what is stored for it, and nothing of another job, another
//!   workspace or another Region;
//! * a page sequence partitions the listing exactly once, whatever the page size
//!   and however many resources the job holds;
//! * the page size is the one THIS operation documents (a default of 50 and a
//!   minimum of 0), bounded by what the service serves, with a `nextToken`
//!   exactly when rows are left over;
//! * only the filter combinations the `filters` member documents are accepted,
//!   each narrowing the page to the resources that satisfy every filter in it,
//!   and a filter's enumerated value must be one the model admits;
//! * only a token this listing minted over this job resumes it;
//! * a workspace or a sync job this service does not hold is refused with the
//!   `ValidationException` this operation models, carrying the message this
//!   service publishes for that miss;
//! * listing REPORTS: it writes nothing at all, not even the lifecycle
//!   transition the clock owes the job.
//!
//! A sync resource is written by a synchronisation — reading the sync source and
//! creating, updating or deleting the corresponding resources in the workspace —
//! which this emulator does not perform, so no operation of it writes one and a
//! job accepted here holds none (`state::SyncResource`). The rows the cases below
//! are about are therefore seeded straight into the store the handlers read (the
//! seam `harness::Emulator::state` exists for, as `sync_job_listing` seeds the
//! jobs its create path cannot build), while the rule under test is about the
//! resources the addressed job HOLDS, whatever wrote them — and one suite covers
//! the job the create path CAN build, which holds none.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::list_sync_resources::{
    DEFAULT_PAGE_SIZE, MAX_FILTERS,
};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::{
    ErrorDetails, SYNC_RESOURCE_STATE_VALUES, SYNC_RESOURCE_TYPE_VALUES,
};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    workspace_not_found_message, SyncResource, SYNC_JOB_ERROR_CODE, SYNC_JOB_NOT_FOUND_MESSAGE,
    SYNC_JOB_SETTLE_SPAN, SYNC_JOB_STATE_CREATING, SYNC_JOB_STATE_ERROR, SYNC_SOURCE_SITEWISE,
    MAX_PAGE_SIZE,
};

use crate::harness::{seed_sync_job, Emulator, Resp, REGION, SYNC_ROLE};

/// A second sync source, for the cases about a listing seeing only the job it
/// addresses. The create path supports one source, so a job under any other is
/// seeded (`harness::seed_sync_job`).
const OTHER_SOURCE: &str = "NEIGHBOUR";

/// One sync resource a case seeds: the row a synchronisation would have written,
/// and so exactly what the listing must report for it.
#[derive(Debug, Clone, Copy)]
struct Seeded {
    /// The resource's identity in the sync SOURCE, which is what the job holds
    /// it under and the order a page is served in.
    external_id: &'static str,
    resource_type: &'static str,
    /// Its id in the WORKSPACE, absent for a resource the synchronisation has
    /// created nothing there for yet.
    resource_id: Option<&'static str>,
    state: &'static str,
    /// The `ErrorCode` this resource's synchronisation failed with, absent while
    /// nothing has failed.
    error_code: Option<&'static str>,
}

/// The seeded set the filter and paging cases run over.
///
/// The rows span what the listing reports about one resource: both
/// `SyncResourceType` values, every `SyncResourceState` value (asserted against
/// the model's own value lists in
/// [`a_listing_reports_the_resources_the_addressed_job_holds`]), a resource with
/// and without a workspace `resourceId`, and one with and without an error —
/// deliberately NOT in `externalId` order, so a page that happened to serve them
/// in insertion order would fail.
const SEEDED: &[Seeded] = &[
    Seeded {
        external_id: "sitewiseModel_beta",
        resource_type: "COMPONENT_TYPE",
        resource_id: Some("betaComponentType"),
        state: "PROCESSING",
        error_code: None,
    },
    Seeded {
        external_id: "sitewiseAsset_alpha",
        resource_type: "ENTITY",
        resource_id: Some("alphaEntity"),
        state: "IN_SYNC",
        error_code: None,
    },
    Seeded {
        external_id: "sitewiseModel_gamma",
        resource_type: "COMPONENT_TYPE",
        resource_id: None,
        state: "DELETED",
        error_code: None,
    },
    Seeded {
        external_id: "sitewiseAsset_beta",
        resource_type: "ENTITY",
        resource_id: None,
        state: "INITIALIZING",
        error_code: None,
    },
    Seeded {
        external_id: "sitewiseModel_alpha",
        resource_type: "COMPONENT_TYPE",
        resource_id: Some("alphaComponentType"),
        state: "IN_SYNC",
        error_code: None,
    },
    Seeded {
        external_id: "sitewiseAsset_gamma",
        resource_type: "ENTITY",
        resource_id: Some("gammaEntity"),
        state: "ERROR",
        error_code: Some("SYNC_CREATING_ERROR"),
    },
];

/// The epoch second the first seeded row's `updateDateTime` sits at; each
/// further row is one second later, so a case can tell the rows apart by the
/// instant the listing reports for each.
const SEED_EPOCH_SECONDS: u64 = 1_700_000_000;

/// The instant seeded for the row at `index`.
fn seeded_instant(index: usize) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(SEED_EPOCH_SECONDS + index as u64)
}

/// That instant as the response carries it: epoch seconds.
fn seeded_epoch_seconds(index: usize) -> f64 {
    (SEED_EPOCH_SECONDS + index as u64) as f64
}

/// The message a seeded failure carries, so a case can tell a reported error
/// apart from an invented one.
fn seeded_error_message(external_id: &str) -> String {
    format!("the synchronisation of {external_id} failed")
}

/// An emulator holding the workspaces `workspace_ids` names.
async fn emulator_with_workspaces(workspace_ids: &[&str]) -> Emulator {
    let emulator = Emulator::new();
    for workspace_id in workspace_ids {
        assert_eq!(
            emulator.create_workspace(workspace_id).await.status,
            200,
            "the workspace {workspace_id:?} the cases name must exist"
        );
    }
    emulator
}

/// Store `rows` as the resources the sync job `workspace_id`/`sync_source`
/// holds, written into the very store the handlers read.
///
/// The job itself must already be there; what a synchronisation would have
/// written is seeded because no operation of this emulator writes one (see this
/// module's docs).
fn seed_sync_resources(
    emulator: &Emulator,
    workspace_id: &str,
    sync_source: &str,
    rows: &[Seeded],
) {
    let state = emulator.state();
    let mut state = state.lock();
    // Seeded into the store of the Region every request from this harness is
    // signed for, the one the handlers serve those requests out of.
    let state = state.region_mut(REGION);
    let workspace = state
        .workspaces
        .get_mut(workspace_id)
        .unwrap_or_else(|| panic!("the workspace {workspace_id:?} must exist before it is seeded"));
    let job = workspace
        .sync_jobs
        .get_mut(sync_source)
        .unwrap_or_else(|| panic!("the sync job {sync_source:?} must exist before it is seeded"));
    for (index, row) in rows.iter().enumerate() {
        job.sync_resources.insert(
            row.external_id.to_string(),
            SyncResource {
                resource_type: row.resource_type.to_string(),
                external_id: row.external_id.to_string(),
                resource_id: row.resource_id.map(str::to_string),
                state: row.state.to_string(),
                error: row.error_code.map(|code| ErrorDetails {
                    code: Some(code.to_string()),
                    message: Some(seeded_error_message(row.external_id)),
                }),
                update_date_time: seeded_instant(index),
            },
        );
    }
}

/// Store `count` generated resources as the ones the sync job
/// `workspace_id`/`sync_source` holds, and answer their external ids in the
/// order a listing must serve them.
///
/// The ids are zero-padded, so they sort the way their numbers do and a page
/// boundary is the obvious one. Used where a case needs MORE resources than the
/// hand-written table carries — a page size this operation documents — and what
/// each row reports is covered by the table instead.
fn seed_generated_sync_resources(
    emulator: &Emulator,
    workspace_id: &str,
    sync_source: &str,
    count: usize,
) -> Vec<String> {
    let state = emulator.state();
    let mut state = state.lock();
    let state = state.region_mut(REGION);
    let workspace = state
        .workspaces
        .get_mut(workspace_id)
        .unwrap_or_else(|| panic!("the workspace {workspace_id:?} must exist before it is seeded"));
    let job = workspace
        .sync_jobs
        .get_mut(sync_source)
        .unwrap_or_else(|| panic!("the sync job {sync_source:?} must exist before it is seeded"));
    let mut external_ids = Vec::new();
    for index in 0..count {
        let external_id = format!("sitewiseAsset{index:03}");
        job.sync_resources.insert(
            external_id.clone(),
            SyncResource {
                resource_type: "ENTITY".to_string(),
                external_id: external_id.clone(),
                resource_id: None,
                state: "INITIALIZING".to_string(),
                error: None,
                update_date_time: seeded_instant(index),
            },
        );
        external_ids.push(external_id);
    }
    external_ids.sort();
    external_ids
}

/// One emulator holding a workspace whose `SITEWISE` job holds [`SEEDED`], plus
/// the neighbours a case must not see: another source in the same workspace and
/// the same source in another workspace, each holding a row of its own.
async fn emulator_with_seeded_job(workspace_id: &str, other_workspace_id: &str) -> Emulator {
    let emulator = emulator_with_workspaces(&[workspace_id, other_workspace_id]).await;
    seed_sync_job(&emulator, workspace_id, SYNC_SOURCE_SITEWISE);
    seed_sync_job(&emulator, workspace_id, OTHER_SOURCE);
    seed_sync_job(&emulator, other_workspace_id, SYNC_SOURCE_SITEWISE);
    seed_sync_resources(&emulator, workspace_id, SYNC_SOURCE_SITEWISE, SEEDED);
    seed_sync_resources(&emulator, workspace_id, OTHER_SOURCE, &[NEIGHBOUR_ROW]);
    seed_sync_resources(
        &emulator,
        other_workspace_id,
        SYNC_SOURCE_SITEWISE,
        &[NEIGHBOUR_ROW],
    );
    emulator
}

/// The one row every neighbouring job holds: an external id no row of [`SEEDED`]
/// carries, so a listing that reported it would be reporting another job's
/// resource.
const NEIGHBOUR_ROW: Seeded = Seeded {
    external_id: "neighbourResource",
    resource_type: "ENTITY",
    resource_id: Some("neighbourEntity"),
    state: "IN_SYNC",
    error_code: None,
};

/// The `syncResources` of a response: a present array, never an absent member,
/// even for a job holding no resource.
fn rows_of(case: &str, resp: &Resp) -> Vec<Value> {
    resp.member("syncResources")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: syncResources must be a present array, got {resp:?}")
        })
}

/// The `externalId`s of a page, in the order the page served them.
fn page_external_ids(case: &str, resp: &Resp) -> Vec<String> {
    rows_of(case, resp)
        .iter()
        .map(|row| {
            row["externalId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every row carries an externalId, got {row:?}")
                })
                .to_string()
        })
        .collect()
}

/// The seeded rows in the order a listing must serve them: `externalId` order.
fn seeded_in_order() -> Vec<Seeded> {
    let mut rows = SEEDED.to_vec();
    rows.sort_by_key(|row| row.external_id);
    rows
}

/// The seeded external ids in that order.
fn seeded_external_ids() -> Vec<String> {
    seeded_in_order()
        .iter()
        .map(|row| row.external_id.to_string())
        .collect()
}

/// The index a row was seeded at, which is the instant seeded for it.
fn seeded_index(external_id: &str) -> usize {
    SEEDED
        .iter()
        .position(|row| row.external_id == external_id)
        .unwrap_or_else(|| panic!("{external_id:?} is a seeded row"))
}

/// Assert `row` reports exactly what was seeded for `seeded`.
fn assert_reports_seeded(case: &str, row: &Value, seeded: &Seeded) {
    assert_eq!(
        (
            row["externalId"].as_str(),
            row["resourceType"].as_str(),
            row["resourceId"].as_str(),
            row["status"]["state"].as_str(),
        ),
        (
            Some(seeded.external_id),
            Some(seeded.resource_type),
            seeded.resource_id,
            Some(seeded.state),
        ),
        "case {case:?}: the row must report what is stored for {:?}, got {row:?}",
        seeded.external_id
    );
    assert_eq!(
        row["updateDateTime"].as_f64(),
        Some(seeded_epoch_seconds(seeded_index(seeded.external_id))),
        "case {case:?}: the row must report the stored updateDateTime of {:?}, got {row:?}",
        seeded.external_id
    );
    // The `error` structure is reported WHOLE, the way every status of this
    // service is: a resource that has not failed carries one with neither a code
    // nor a message, so a client that reads `status.error` needs no null guard.
    assert!(
        row["status"]["error"].is_object(),
        "case {case:?}: the status of {:?} must carry an error structure, got {row:?}",
        seeded.external_id
    );
    let expected_message = seeded
        .error_code
        .map(|_| seeded_error_message(seeded.external_id));
    assert_eq!(
        (
            row["status"]["error"]["code"].as_str(),
            row["status"]["error"]["message"].as_str(),
        ),
        (seeded.error_code, expected_message.as_deref()),
        "case {case:?}: the reported error of {:?} must be the stored one, got {row:?}",
        seeded.external_id
    );
}

/// RULE: a listing reports the resources the sync job the request ADDRESSES
/// holds — looked up by the workspace and the sync source together — in
/// `externalId` order, each row reporting exactly what is stored for it, and
/// nothing of a job another source or another workspace holds.
#[tokio::test]
async fn a_listing_reports_the_resources_the_addressed_job_holds() {
    const WORKSPACE: &str = "listedWorkspace";
    const OTHER_WORKSPACE: &str = "otherListedWorkspace";
    let emulator = emulator_with_seeded_job(WORKSPACE, OTHER_WORKSPACE).await;

    // The seeded rows span the model's own value lists
    // (`ops::types::SYNC_RESOURCE_STATE_VALUES` /
    // `SYNC_RESOURCE_TYPE_VALUES`, mirroring the `SyncResourceState` and
    // `SyncResourceType` enums), so the cases below are not about one state or
    // one kind of resource.
    for state in SYNC_RESOURCE_STATE_VALUES {
        assert!(
            SEEDED.iter().any(|row| row.state == *state),
            "the seeded rows must cover the modelled state {state:?}"
        );
    }
    for resource_type in SYNC_RESOURCE_TYPE_VALUES {
        assert!(
            SEEDED.iter().any(|row| row.resource_type == *resource_type),
            "the seeded rows must cover the modelled resourceType {resource_type:?}"
        );
    }

    let case = "allHeld";
    let listed = emulator
        .list_sync_resources(
            WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            json!({"maxResults": MAX_PAGE_SIZE}),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListSyncResources, got {listed:?}"
    );
    assert_eq!(
        page_external_ids(case, &listed),
        seeded_external_ids(),
        "case {case:?}: the listing must report every resource the job holds in externalId \
         order, got {listed:?}"
    );
    assert!(
        listed.member("nextToken").is_null(),
        "case {case:?}: the whole set fits in one page, so no token is minted, got {listed:?}"
    );
    for (row, seeded) in rows_of(case, &listed).iter().zip(seeded_in_order()) {
        assert_reports_seeded(case, row, &seeded);
    }

    // The neighbours: a listing addresses ONE job, so each reports only its own
    // row.
    for (case, workspace_id, sync_source) in [
        ("otherSource", WORKSPACE, OTHER_SOURCE),
        ("otherWorkspace", OTHER_WORKSPACE, SYNC_SOURCE_SITEWISE),
    ] {
        let listed = emulator
            .list_sync_resources(workspace_id, sync_source, json!({}))
            .await;
        assert_eq!(
            page_external_ids(case, &listed),
            vec![NEIGHBOUR_ROW.external_id.to_string()],
            "case {case:?}: a listing reports only the resources ITS job holds, got {listed:?}"
        );
    }
}

/// RULE: a workspace or a sync job this service does not hold has no resources
/// to list, and this operation models no `ResourceNotFoundException`, so the
/// refusal is the `ValidationException` it does model, carrying the message this
/// service publishes for that miss.
///
/// The rows differ along WHICH half of the address misses: a workspace that was
/// never created, and a workspace that exists but syncs from no such source —
/// including a source this service does not support at all.
#[tokio::test]
async fn an_absent_workspace_or_sync_job_is_refused() {
    const WORKSPACE: &str = "refusedWorkspace";
    const OTHER_WORKSPACE: &str = "otherRefusedWorkspace";
    let emulator = emulator_with_seeded_job(WORKSPACE, OTHER_WORKSPACE).await;

    for case in ["neverCreated", "absentWorkspace0"] {
        let refused = emulator
            .list_sync_resources(case, SYNC_SOURCE_SITEWISE, json!({}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: listing a job of an absent workspace must be refused, got {refused:?}"
        );
        assert_eq!(
            refused.member("message").as_str(),
            Some(workspace_not_found_message(case).as_str()),
            "case {case:?}: the refusal must name the workspace that was looked for, got \
             {refused:?}"
        );
    }

    // A workspace this service DOES hold, under a source it holds no job for —
    // whether or not the service supports that source.
    for sync_source in ["ABSENT_SOURCE", "SITEWISE_2", "sitewise"] {
        let refused = emulator
            .list_sync_resources(WORKSPACE, sync_source, json!({}))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {sync_source:?}: listing a job the workspace does not hold must be refused, \
             got {refused:?}"
        );
        assert_eq!(
            refused.member("message").as_str(),
            Some(SYNC_JOB_NOT_FOUND_MESSAGE),
            "case {sync_source:?}: expected {SYNC_JOB_NOT_FOUND_MESSAGE:?}, got {refused:?}"
        );
    }
}

/// Walk every page of the listing for one job at page size `max_results`,
/// returning the external ids in the order they were served (so a duplicate is
/// visible) and the number of pages.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    workspace_id: &str,
    max_results: Option<u32>,
) -> (Vec<String>, usize) {
    assert_ne!(
        max_results,
        Some(0),
        "case {case:?}: a page size of zero never advances, so a walk cannot be driven at it"
    );
    let mut external_ids = Vec::new();
    let mut pages = 0;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let page = emulator
            .list_sync_resources(workspace_id, SYNC_SOURCE_SITEWISE, request)
            .await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected 200 from ListSyncResources, got {page:?}"
        );
        pages += 1;
        let served = page_external_ids(case, &page);
        let page_size = max_results.map_or(DEFAULT_PAGE_SIZE, |size| size as usize);
        assert!(
            served.len() <= page_size,
            "case {case:?}: a page must not exceed the page size {page_size}, got {served:?}"
        );
        external_ids.extend(served);
        next_token = page
            .member("nextToken")
            .as_str()
            .map(|token| token.to_string());
        if next_token.is_none() {
            return (external_ids, pages);
        }
        assert!(
            pages <= SEEDED.len() + 2,
            "case {case:?}: the walk must terminate, got {pages} pages"
        );
    }
}

/// RULE: a page sequence partitions the listing — every resource the job holds
/// is served exactly once, in `externalId` order, no page is larger than the page
/// size, and only the last page carries no `nextToken`.
///
/// The rows are the page sizes: one, several that do and do not divide the set,
/// exactly the set's size, one past it, and the top of the published range.
#[tokio::test]
async fn a_page_sequence_partitions_the_listing() {
    const WORKSPACE: &str = "pagedWorkspace";
    const OTHER_WORKSPACE: &str = "otherPagedWorkspace";
    let emulator = emulator_with_seeded_job(WORKSPACE, OTHER_WORKSPACE).await;
    let expected = seeded_external_ids();

    for max_results in [None, Some(1), Some(2), Some(4), Some(6), Some(7), Some(200)] {
        let case = format!("maxResults={max_results:?}");
        let (served, pages) = walk_pages(&emulator, &case, WORKSPACE, max_results).await;
        assert_eq!(
            served, expected,
            "case {case:?}: the pages must partition the listing in externalId order, got \
             {served:?}"
        );
        let page_size = max_results.map_or(DEFAULT_PAGE_SIZE, |size| size as usize);
        let expected_pages = expected.len().div_ceil(page_size).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: {} rows at a page size of {page_size} is {expected_pages} pages, \
             got {pages}",
            expected.len()
        );
    }
}

/// RULE: the page size is the one THIS operation documents and bounds — a request
/// naming no `maxResults` is served the documented default of 50, a value inside
/// the published range is served as asked, the documented minimum of 0 is a page
/// of no rows rather than a refusal, a value at the top of the range serves the
/// whole set, and a value past the published maximum is refused — with a
/// `nextToken` exactly when rows are left over.
///
/// The rows are the published bounds of this member (0 and 200), one past the
/// published maximum (201), values inside the range, and the request that names
/// none, taken over a job holding MORE than the documented default so the default
/// itself is observable.
#[tokio::test]
async fn the_page_size_is_this_operations_documented_one_and_is_bounded() {
    const WORKSPACE: &str = "sizedWorkspace";
    const EMPTY_WORKSPACE: &str = "emptySizedWorkspace";
    // More resources than the documented default, so a request naming no
    // `maxResults` is served a page the default decides rather than the whole
    // set.
    const HELD: usize = DEFAULT_PAGE_SIZE + 10;

    // The documented number itself, pinned as a literal so the rows below cannot
    // drift with the implementation: this operation's `maxResults` documents
    // "The default is 50 … Minimum value of 0. Maximum value of 200."
    assert_eq!(
        DEFAULT_PAGE_SIZE, 50,
        "this operation's documented default page size is 50, got {DEFAULT_PAGE_SIZE}"
    );

    let emulator = emulator_with_workspaces(&[WORKSPACE, EMPTY_WORKSPACE]).await;
    seed_sync_job(&emulator, WORKSPACE, SYNC_SOURCE_SITEWISE);
    seed_sync_job(&emulator, EMPTY_WORKSPACE, SYNC_SOURCE_SITEWISE);
    let all = seed_generated_sync_resources(&emulator, WORKSPACE, SYNC_SOURCE_SITEWISE, HELD);

    // (the request, the rows it must serve, whether a token must come back)
    let rows: Vec<(String, Value, usize, bool)> = vec![
        ("default".to_string(), json!({}), DEFAULT_PAGE_SIZE, true),
        ("one".to_string(), json!({"maxResults": 1}), 1, true),
        (
            "underTheSet".to_string(),
            json!({"maxResults": HELD - 1}),
            HELD - 1,
            true,
        ),
        (
            "exactlyTheSet".to_string(),
            json!({"maxResults": HELD}),
            HELD,
            false,
        ),
        (
            "topOfRange".to_string(),
            json!({"maxResults": MAX_PAGE_SIZE}),
            HELD,
            false,
        ),
        // The documented MINIMUM: a page of no rows, and a token, because every
        // row is still left over.
        ("zero".to_string(), json!({"maxResults": 0}), 0, true),
    ];
    for (case, request, expected_rows, expects_token) in &rows {
        let page = emulator
            .list_sync_resources(WORKSPACE, SYNC_SOURCE_SITEWISE, request.clone())
            .await;
        assert_eq!(
            (
                page.status,
                page_external_ids(case, &page).len(),
                !page.member("nextToken").is_null(),
            ),
            (200, *expected_rows, *expects_token),
            "case {case:?}: expected {expected_rows} rows and a token={expects_token}, got {page:?}"
        );
        assert_eq!(
            page_external_ids(case, &page).as_slice(),
            &all[..*expected_rows],
            "case {case:?}: a page starts at the beginning of the set, got {page:?}"
        );
    }

    // The documented minimum on a job holding NOTHING: an empty page and no
    // token, because there is nothing left over to resume at.
    let empty = emulator
        .list_sync_resources(EMPTY_WORKSPACE, SYNC_SOURCE_SITEWISE, json!({"maxResults": 0}))
        .await;
    assert_eq!(
        (
            empty.status,
            rows_of("emptyJobAtZero", &empty).len(),
            empty.member("nextToken").is_null(),
        ),
        (200, 0, true),
        "case \"emptyJobAtZero\": a job holding no resource lists an empty array and no token, \
         got {empty:?}"
    );

    // One past the published maximum, where the modelled range refuses it before
    // the handler runs.
    for past_bound in [201u32, 1000] {
        let refused = emulator
            .list_sync_resources(
                WORKSPACE,
                SYNC_SOURCE_SITEWISE,
                json!({"maxResults": past_bound}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case Some({past_bound}): a maxResults past the published range must be refused, got \
             {refused:?}"
        );
    }
}

/// The external ids of the seeded rows that satisfy every filter in
/// `combination`, in the order a listing must serve them — the expectation
/// computed from the seeded table rather than written out case by case.
fn expected_for(combination: &[(&str, &str)]) -> Vec<String> {
    seeded_in_order()
        .iter()
        .filter(|row| {
            combination.iter().all(|(member, wanted)| match *member {
                "state" => row.state == *wanted,
                "resourceType" => row.resource_type == *wanted,
                "resourceId" => row.resource_id == Some(*wanted),
                "externalId" => row.external_id == *wanted,
                other => panic!("{other:?} is no filter member"),
            })
        })
        .map(|row| row.external_id.to_string())
        .collect()
}

/// `filters` as the wire carries a combination: one single-member object per
/// filter, in the order given.
fn filters_body(combination: &[(&str, &str)]) -> Value {
    Value::Array(
        combination
            .iter()
            .map(|(member, wanted)| json!({*member: *wanted}))
            .collect(),
    )
}

/// RULE: every filter combination the member documents is served, narrowed to the
/// resources that satisfy EVERY filter in it, whatever order the filters are
/// listed in.
///
/// The rows are every documented combination over values drawn from the seeded
/// set: each modelled state, each resource type crossed with every stored
/// `resourceId` and `externalId`, and ids no resource carries — each expectation
/// computed from the seeded table.
#[tokio::test]
async fn each_documented_filter_combination_narrows_the_page() {
    const WORKSPACE: &str = "filteredWorkspace";
    const OTHER_WORKSPACE: &str = "otherFilteredWorkspace";
    let emulator = emulator_with_seeded_job(WORKSPACE, OTHER_WORKSPACE).await;

    let mut combinations: Vec<Vec<(&str, &str)>> = vec![vec![]];
    for state in SYNC_RESOURCE_STATE_VALUES {
        combinations.push(vec![("state", state)]);
    }
    // `resourceId` and `externalId` are only filtered beside a `resourceType`,
    // which is what the member documents; both orders are sent, since a
    // combination is a SET of members.
    let resource_ids: Vec<&str> = SEEDED
        .iter()
        .filter_map(|row| row.resource_id)
        // An id no resource carries: a filter that selects nothing is served an
        // empty page, not a refusal.
        .chain(["absentResourceId"])
        .collect();
    let external_ids: Vec<&str> = SEEDED
        .iter()
        .map(|row| row.external_id)
        .chain(["absentExternalId"])
        .collect();
    for resource_type in SYNC_RESOURCE_TYPE_VALUES {
        for resource_id in &resource_ids {
            combinations.push(vec![
                ("resourceType", resource_type),
                ("resourceId", resource_id),
            ]);
            combinations.push(vec![
                ("resourceId", resource_id),
                ("resourceType", resource_type),
            ]);
        }
        for external_id in &external_ids {
            combinations.push(vec![
                ("resourceType", resource_type),
                ("externalId", external_id),
            ]);
            combinations.push(vec![
                ("externalId", external_id),
                ("resourceType", resource_type),
            ]);
        }
    }

    for combination in &combinations {
        let case = format!("{combination:?}");
        let request = json!({"maxResults": MAX_PAGE_SIZE, "filters": filters_body(combination)});
        let listed = emulator
            .list_sync_resources(WORKSPACE, SYNC_SOURCE_SITEWISE, request)
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: a documented filter combination must be served, got {listed:?}"
        );
        assert_eq!(
            page_external_ids(&case, &listed),
            expected_for(combination),
            "case {case:?}: the page must be the resources satisfying every filter, got {listed:?}"
        );
    }

    // A filtered listing pages the way an unfiltered one does: the filter
    // decides the SET, the page size decides how much of it one call serves.
    let in_sync = vec![("state", "IN_SYNC")];
    let expected = expected_for(&in_sync);
    assert!(
        expected.len() > 1,
        "the seeded set must hold more than one IN_SYNC resource for this case, got {expected:?}"
    );
    let first = emulator
        .list_sync_resources(
            WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            json!({"maxResults": 1, "filters": filters_body(&in_sync)}),
        )
        .await;
    let token = first
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("case \"filteredPaging\": a page with rows left over mints a token, got {first:?}"))
        .to_string();
    let second = emulator
        .list_sync_resources(
            WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            json!({"maxResults": 1, "filters": filters_body(&in_sync), "nextToken": token}),
        )
        .await;
    assert_eq!(
        [
            page_external_ids("filteredPaging", &first),
            page_external_ids("filteredPaging", &second),
        ]
        .concat(),
        expected,
        "case \"filteredPaging\": the pages of a filtered listing partition the filtered set, \
         got {first:?} then {second:?}"
    );
}

/// RULE: a filter combination the member does not document is refused with the
/// modelled `ValidationException`, naming what the caller sent and the
/// combinations it may send instead.
///
/// The rows differ along how the combination is wrong: a member the documentation
/// never names alone, two members no documented combination pairs, one member
/// named twice, and more filters than any documented combination has.
#[tokio::test]
async fn a_filter_combination_outside_the_documented_set_is_refused() {
    const WORKSPACE: &str = "comboWorkspace";
    const OTHER_WORKSPACE: &str = "otherComboWorkspace";
    let emulator = emulator_with_seeded_job(WORKSPACE, OTHER_WORKSPACE).await;

    let rows: Vec<Vec<(&str, &str)>> = vec![
        vec![("resourceType", "ENTITY")],
        vec![("resourceId", "alphaEntity")],
        vec![("externalId", "sitewiseAsset_alpha")],
        vec![("state", "IN_SYNC"), ("resourceType", "ENTITY")],
        vec![("state", "IN_SYNC"), ("resourceId", "alphaEntity")],
        vec![("state", "IN_SYNC"), ("externalId", "sitewiseAsset_alpha")],
        vec![("state", "IN_SYNC"), ("state", "ERROR")],
        vec![("resourceType", "ENTITY"), ("resourceType", "ENTITY")],
        vec![
            ("resourceId", "alphaEntity"),
            ("externalId", "sitewiseAsset_alpha"),
        ],
        // More filters than any documented combination names.
        vec![
            ("resourceType", "ENTITY"),
            ("resourceId", "alphaEntity"),
            ("externalId", "sitewiseAsset_alpha"),
        ],
    ];

    for combination in &rows {
        let case = format!("{combination:?}");
        assert!(
            combination.len() <= MAX_FILTERS + 1,
            "case {case:?}: the rows stay at the bound this listing accepts plus one"
        );
        let refused = emulator
            .list_sync_resources(
                WORKSPACE,
                SYNC_SOURCE_SITEWISE,
                json!({"filters": filters_body(combination)}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a combination outside the documented set must be refused, got \
             {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains("filters")),
            "case {case:?}: the refusal must name the member it is about, got {refused:?}"
        );
    }
}

/// RULE: a filter's enumerated value must be one the model admits — `state` a
/// `SyncResourceState` and `resourceType` a `SyncResourceType` — and a value
/// outside that list is refused with the modelled `ValidationException` naming
/// the member, the value and the values admitted, rather than quietly selecting
/// no resource.
///
/// The rows differ along how the value misses: a value from ANOTHER enum of this
/// service, the right value in the wrong case, one with trailing space, and the
/// empty string.
#[tokio::test]
async fn a_filter_names_only_values_the_model_admits() {
    const WORKSPACE: &str = "enumWorkspace";
    const OTHER_WORKSPACE: &str = "otherEnumWorkspace";
    let emulator = emulator_with_seeded_job(WORKSPACE, OTHER_WORKSPACE).await;

    // (the member, the value, the admitted list the refusal must name)
    let mut rows: Vec<(&str, &str, &[&str])> = Vec::new();
    for bad in ["ACTIVE", "in_sync", "IN_SYNC ", ""] {
        rows.push(("state", bad, SYNC_RESOURCE_STATE_VALUES));
    }
    for bad in ["SCENE", "entity", "ENTITY ", ""] {
        rows.push(("resourceType", bad, SYNC_RESOURCE_TYPE_VALUES));
    }

    for (member, value, admitted) in &rows {
        let case = format!("{member}={value:?}");
        // Sent in a combination the member DOES document, so what refuses the
        // request is the value and nothing else: a bad `resourceType` travels
        // beside a `resourceId`.
        let combination: Vec<(&str, &str)> = match *member {
            "state" => vec![("state", value)],
            _ => vec![("resourceType", value), ("resourceId", "alphaEntity")],
        };
        let refused = emulator
            .list_sync_resources(
                WORKSPACE,
                SYNC_SOURCE_SITEWISE,
                json!({"filters": filters_body(&combination)}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a value outside the modelled list must be refused, got {refused:?}"
        );
        let message = refused
            .member("message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(member) && admitted.iter().all(|value| message.contains(value)),
            "case {case:?}: the refusal must name the member and the values the model admits, \
             got {message:?}"
        );
    }
}

/// RULE: a `nextToken` is only resumable as the token this listing minted over
/// THIS job — an absent or empty one starts at the beginning, and anything else
/// is refused with the modelled `ValidationException` rather than silently
/// resumed.
///
/// The rows differ along how the token is wrong: not a token at all, one mangled
/// in transit, ones minted by this service's OTHER listings, ones this listing
/// minted over a different workspace or a different sync source, and one whose
/// envelope is right but whose resume key is empty.
#[tokio::test]
async fn only_a_token_this_listing_minted_over_this_job_resumes_it() {
    const WORKSPACE: &str = "tokenWorkspace";
    const OTHER_WORKSPACE: &str = "otherTokenWorkspace";
    let emulator = emulator_with_seeded_job(WORKSPACE, OTHER_WORKSPACE).await;
    let expected = seeded_external_ids();

    // An absent and an empty token both mean "the first page".
    let first = emulator
        .list_sync_resources(WORKSPACE, SYNC_SOURCE_SITEWISE, json!({"maxResults": 2}))
        .await;
    let from_empty = emulator
        .list_sync_resources(
            WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            json!({"maxResults": 2, "nextToken": ""}),
        )
        .await;
    assert_eq!(
        page_external_ids("emptyToken", &from_empty),
        page_external_ids("firstPage", &first),
        "case \"emptyToken\": an empty token starts at the beginning, got {from_empty:?}"
    );

    let ours = first
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("the first page of a longer listing mints a token, got {first:?}"))
        .to_string();
    let other_workspaces_token = emulator
        .list_sync_resources(
            OTHER_WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            json!({"maxResults": 0}),
        )
        .await
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("a page of no rows over a held job names where it stands"))
        .to_string();
    let other_sources_token = emulator
        .list_sync_resources(WORKSPACE, OTHER_SOURCE, json!({"maxResults": 0}))
        .await
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("a page of no rows over a held job names where it stands"))
        .to_string();

    let rows: Vec<(&str, String)> = vec![
        ("notHex", "not-a-token".to_string()),
        ("mangled", format!("{ours}ff")),
        // Hex that decodes, but carries no envelope at all.
        (
            "noEnvelope",
            hex::encode(format!("{WORKSPACE}\n{SYNC_SOURCE_SITEWISE}")),
        ),
        (
            "syncJobsListingToken",
            hex::encode(format!("iottwinmaker:syncJobs:1:SITEWISE\n{WORKSPACE}")),
        ),
        (
            "entityListingToken",
            hex::encode(format!("iottwinmaker:entities:1:pump\n{WORKSPACE}")),
        ),
        // This listing's token, minted over another workspace and over another
        // sync source of the same workspace.
        ("anotherWorkspacesToken", other_workspaces_token),
        ("anotherSourcesToken", other_sources_token),
        // Our own envelope and scope, with no resume key inside it.
        (
            "emptyResumeKey",
            hex::encode(format!(
                "iottwinmaker:syncResources:1:\n{WORKSPACE}\n{SYNC_SOURCE_SITEWISE}"
            )),
        ),
    ];

    for (case, token) in &rows {
        let refused = emulator
            .list_sync_resources(
                WORKSPACE,
                SYNC_SOURCE_SITEWISE,
                json!({"maxResults": 2, "nextToken": token}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a token this listing did not mint over this job must be refused, got \
             {refused:?}"
        );
    }

    // Our own token still resumes, so what refused the rows above is the token
    // and nothing else.
    let resumed = emulator
        .list_sync_resources(
            WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            json!({"maxResults": 2, "nextToken": ours}),
        )
        .await;
    assert_eq!(
        page_external_ids("ourToken", &resumed),
        expected[2..4].to_vec(),
        "case \"ourToken\": the token must resume where the page stopped, got {resumed:?}"
    );
}

/// RULE: a listing REPORTS — it writes nothing a later caller sees, not even the
/// lifecycle transition the clock owes the JOB, whose status no member of this
/// response carries.
#[tokio::test]
async fn a_listing_reports_and_writes_nothing() {
    const WORKSPACE: &str = "reportingWorkspace";
    const OTHER_WORKSPACE: &str = "otherReportingWorkspace";
    let emulator = emulator_with_seeded_job(WORKSPACE, OTHER_WORKSPACE).await;

    // Past the instant the job's own outcome fell due, so a call that settled it
    // would be observable in the store.
    std::thread::sleep(SYNC_JOB_SETTLE_SPAN + Duration::from_millis(100));

    let before = emulator
        .list_sync_resources(
            WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            json!({"maxResults": MAX_PAGE_SIZE}),
        )
        .await;
    let again = emulator
        .list_sync_resources(
            WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            json!({"maxResults": MAX_PAGE_SIZE}),
        )
        .await;
    assert_eq!(
        (before.status, &before.body),
        (again.status, &again.body),
        "case \"twice\": two listings of an unchanged job must answer the same, got {before:?} \
         then {again:?}"
    );

    let (state, resources) = {
        let state = emulator.state();
        let state = state.lock();
        let job = state
            .region(REGION)
            .workspaces
            .get(WORKSPACE)
            .and_then(|workspace| workspace.sync_jobs.get(SYNC_SOURCE_SITEWISE))
            .unwrap_or_else(|| panic!("the seeded job must still be there"));
        (job.state.clone(), job.sync_resources.len())
    };
    assert_eq!(
        (state.as_str(), resources),
        (SYNC_JOB_STATE_CREATING, SEEDED.len()),
        "case \"storeUnchanged\": this listing reports no status of the job, so it materialises \
         none and changes no resource, got {state:?} over {resources} resources"
    );

    // And the operation that DOES report the job's status still materialises the
    // transition the clock owed, so what the listing left alone is reachable.
    let read = emulator
        .get_sync_job(Some(WORKSPACE), SYNC_SOURCE_SITEWISE)
        .await;
    assert_eq!(
        read.member("status")["state"].as_str(),
        Some(SYNC_JOB_STATE_ERROR),
        "case \"readAfterListing\": the read reports the outcome the clock owed, got {read:?}"
    );
}

/// RULE: a listing is answered out of the store of the Region its request is
/// signed for — a job created through one Region's endpoint, and the resources it
/// holds, are invisible through another's.
#[tokio::test]
async fn a_listing_is_answered_out_of_the_region_it_is_signed_for() {
    const WORKSPACE: &str = "regionWorkspace";
    const OTHER_WORKSPACE: &str = "otherRegionWorkspace";
    let emulator = emulator_with_seeded_job(WORKSPACE, OTHER_WORKSPACE).await;

    let here = emulator
        .list_sync_resources(
            WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            json!({"maxResults": MAX_PAGE_SIZE}),
        )
        .await;
    assert_eq!(
        page_external_ids("signedForTheSeededRegion", &here),
        seeded_external_ids(),
        "case \"signedForTheSeededRegion\": the listing reports the seeded rows, got {here:?}"
    );

    for region in [Some("eu-west-1"), Some("ap-south-1"), None] {
        let case = format!("signedFor={region:?}");
        let elsewhere = emulator
            .list_sync_resources_signed_for(region, WORKSPACE, SYNC_SOURCE_SITEWISE, json!({}))
            .await;
        assert_eq!(
            (elsewhere.status, elsewhere.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: another Region holds no such workspace, so the listing is refused, \
             got {elsewhere:?}"
        );
    }
}

/// RULE: the resources a listing reports are the ones the job HOLDS — so the job
/// this emulator's create path accepts, whose synchronisation reads nothing from
/// the source and creates nothing in the workspace, lists a present but empty
/// array and no token, whatever page or filter is asked for.
#[tokio::test]
async fn a_created_job_that_has_synchronised_nothing_lists_an_empty_page() {
    const WORKSPACE: &str = "createdJobWorkspace";
    let emulator = emulator_with_workspaces(&[WORKSPACE]).await;
    let created = emulator
        .create_sync_job(
            WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            json!({"syncRole": SYNC_ROLE}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateSyncJob, got {created:?}"
    );

    let requests: Vec<(&str, Value)> = vec![
        ("noMembers", json!({})),
        ("atTheTopOfTheRange", json!({"maxResults": MAX_PAGE_SIZE})),
        ("atTheDocumentedMinimum", json!({"maxResults": 0})),
        (
            "filteredByState",
            json!({"filters": filters_body(&[("state", "IN_SYNC")])}),
        ),
        (
            "filteredByTypeAndId",
            json!({"filters": filters_body(&[("resourceType", "ENTITY"), ("resourceId", "pump")])}),
        ),
    ];
    for (case, request) in &requests {
        let listed = emulator
            .list_sync_resources(WORKSPACE, SYNC_SOURCE_SITEWISE, request.clone())
            .await;
        assert_eq!(
            (
                listed.status,
                rows_of(case, &listed).len(),
                listed.member("nextToken").is_null(),
            ),
            (200, 0, true),
            "case {case:?}: a job that has synchronised nothing lists an empty array and no \
             token, got {listed:?}"
        );
    }

    // The empty page is what the job's own record says: its outcome, once the
    // clock made it due, is the failure that says nothing was synchronised.
    std::thread::sleep(SYNC_JOB_SETTLE_SPAN + Duration::from_millis(100));
    let read = emulator
        .get_sync_job(Some(WORKSPACE), SYNC_SOURCE_SITEWISE)
        .await;
    assert_eq!(
        (
            read.member("status")["state"].as_str(),
            read.member("status")["error"]["code"].as_str(),
        ),
        (Some(SYNC_JOB_STATE_ERROR), Some(SYNC_JOB_ERROR_CODE)),
        "the job's record must say its synchronisation never ran, got {read:?}"
    );
    let after = emulator
        .list_sync_resources(WORKSPACE, SYNC_SOURCE_SITEWISE, json!({}))
        .await;
    assert_eq!(
        (after.status, rows_of("afterSettling", &after).len()),
        (200, 0),
        "case \"afterSettling\": the job synchronised nothing, so it holds nothing to list, got \
         {after:?}"
    );
}
