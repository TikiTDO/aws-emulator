// SPDX-License-Identifier: CC0-1.0

//! Recommendation report generation, end to end over HTTP.
//!
//! The rule under test is the one the service states for a report generation TASK, not
//! for one request:
//!
//! 1. `StartRecommendationReportGeneration` mints ONE task, `IN_PROGRESS`, answers with
//!    its `RecommendationTaskId`, and `GetRecommendationReportDetails` reports that
//!    stored record — whatever `outputFormat` and however many `groupIdFilter` groups the
//!    request carried;
//! 2. only ONE task runs at a time, so a start call made while one is in progress
//!    COALESCES onto it: it answers HTTP 200 with the id of the task already in flight and
//!    mints nothing, whatever parameters it carries; once the clock has given that task
//!    its terminal status a start call mints a new task;
//! 3. the clock owes the task a terminal status after the completion span, and that
//!    status is `SUCCESS` — the status the service's recorded behaviour shows a started
//!    task settling on — with a status message reporting what was in scope; this account
//!    named no bucket for the reports the service generates, so its tasks write no report
//!    FILE and name no location at all (`s3Bucket` and `s3Keys` stay absent rather than
//!    naming a file nothing wrote — what an account that DID name one reports is
//!    `recommendation_report_file.rs`);
//! 4. an `id` that names no task of this account is refused with the modelled
//!    `ResourceNotFoundException` (HTTP 404), and a request carrying an enum value the
//!    model does not declare with 400 `ValidationException`;
//! 5. the store is bounded: a `groupIdFilter` past the number of entries the operation
//!    accepts is refused with 400 `ValidationException`, and a start call made once the
//!    store holds the task quota with the modelled 429 `ThrottlingException` — neither
//!    minting anything.
//!
//! Each table therefore spans the class the rule generalises over — every `OutputFormat`
//! value and an absent one, a filter of no, zero, one and two groups over both `GroupName`
//! values, and a scope covering the whole portfolio, part of it and none of it — rather
//! than the one recorded pair of calls. Every status value is spelled out here
//! independently of `src/state.rs`, so a wrong constant in the crate fails here rather
//! than agreeing with itself.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::ops::types::Group;
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::state::{
    ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState,
    RecommendationReport, ServerRecord, COMPLETION_SPAN, MAX_GROUP_FILTER_ENTRIES, REPORT_QUOTA,
};
use service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{at, call, emulator, Answer, TEST_REGION};

/// Every value `OutputFormat` admits, from
/// `smithy_query(query="shape",shape="OutputFormat")`.
pub(crate) const OUTPUT_FORMATS: &[&str] = &["Excel", "Json"];

/// Every value `GroupName` admits, from `smithy_query(query="shape",shape="GroupName")`.
pub(crate) const GROUP_NAMES: &[&str] = &["ExternalId", "ExternalSourceType"];

/// Every value `RecommendationReportStatus` admits, from
/// `smithy_query(query="shape",shape="RecommendationReportStatus")`.
const REPORT_STATUSES: &[&str] = &["FAILED", "IN_PROGRESS", "SUCCESS"];

/// The non-terminal `RecommendationReportStatus`, and the terminal one a started task
/// settles on.
pub(crate) const REPORT_IN_PROGRESS: &str = "IN_PROGRESS";
pub(crate) const REPORT_SETTLED: &str = "SUCCESS";

/// `RecommendationReportStatusMessage`'s `@length(0, 512)` and `.*\S.*`, from
/// `smithy_query(query="shape",shape="RecommendationReportDetails")`.
const STATUS_MESSAGE_MAX: usize = 512;

/// `RecommendationTaskId`'s `[0-9a-z-:]+` within its `@length(max: 52)`, checked here
/// rather than through any crate predicate so a wrong id shape cannot make this pass.
fn matches_recommendation_task_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 52
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == '-' || c == ':')
}

/// The wire error code and status a start call is NOT answered with when a report
/// generation task of the portfolio is already in flight: the repeat coalesces onto that
/// task instead of being refused.
pub(crate) const CONFLICT_EXCEPTION: &str = "ConflictException";
pub(crate) const CONFLICT_STATUS: u16 = 409;

pub(crate) async fn start_report(router: &MultiServiceRouter, request: Value) -> Answer {
    call(
        router,
        "POST",
        "/start-recommendation-report-generation",
        Some(request),
    )
    .await
}

pub(crate) async fn report_details(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/get-recommendation-report-details/{id}"),
        None,
    )
    .await
}

/// One `groupIdFilter` entry, held to the `GroupName` catalog above.
pub(crate) fn group(name: &str, value: &str) -> Value {
    assert!(
        GROUP_NAMES.contains(&name),
        "a group name GroupName does not declare: {name:?}"
    );
    json!({"name": name, "value": value})
}

/// The `StartRecommendationReportGeneration` requests every start case runs, differing
/// along what the rule generalises over: whether `outputFormat` is named at all and which
/// value it carries, and how many groups the filter names (none, zero, one, two) over both
/// `GroupName` values.
fn start_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("no members at all", json!({})),
        (
            "the default format named explicitly",
            json!({"outputFormat": OUTPUT_FORMATS[0]}),
        ),
        (
            // The `OutputFormat` entry the issue did not cite.
            "the other output format, with one group",
            json!({
                "outputFormat": OUTPUT_FORMATS[1],
                "groupIdFilter": [group(GROUP_NAMES[0], "wave-1")],
            }),
        ),
        (
            "an empty groupIdFilter",
            json!({"outputFormat": OUTPUT_FORMATS[0], "groupIdFilter": []}),
        ),
        (
            "two groups, one of each GroupName",
            json!({
                "outputFormat": OUTPUT_FORMATS[1],
                "groupIdFilter": [
                    group(GROUP_NAMES[0], "wave-2"),
                    group(GROUP_NAMES[1], "MPA"),
                ],
            }),
        ),
    ]
}

/// One emulator, plus the engine behind it so a case can read the store the handlers
/// write — which is how "a second start call minted nothing" is checked.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// How many report generation tasks the store holds in the Region the calls are signed
/// for.
pub(crate) fn stored_task_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    engine
        .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>()
        .lock()
        .region(TEST_REGION)
        .recommendation_reports
        .len()
}

/// One emulator whose inventory holds `servers` — the same `servers` map an import's
/// ingestion writes, and what a report's scope is counted from.
fn emulator_with(servers: Vec<ServerRecord>) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let portfolio = store.region_entry(TEST_REGION);
        for record in servers {
            portfolio.servers.insert(record.id.clone(), record);
        }
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

fn stored_group(name: &str, value: &str) -> Group {
    Group {
        name: Some(name.to_string()),
        value: Some(value.to_string()),
    }
}

/// The inventory the scope table runs against: two servers in one group, one in another
/// pair of groups, and one that came in with no group at all.
///
/// Ids are `ResourceId`s (`^[0-9a-b]+`, max 44) like the ones an import mints.
fn inventory() -> Vec<ServerRecord> {
    vec![
        ServerRecord {
            id: "0a".to_string(),
            group_id: vec![stored_group(GROUP_NAMES[0], "wave-1")],
            ..Default::default()
        },
        ServerRecord {
            id: "0b".to_string(),
            group_id: vec![stored_group(GROUP_NAMES[0], "wave-1")],
            ..Default::default()
        },
        ServerRecord {
            id: "1a".to_string(),
            group_id: vec![
                stored_group(GROUP_NAMES[0], "wave-2"),
                stored_group(GROUP_NAMES[1], "MPA"),
            ],
            ..Default::default()
        },
        ServerRecord {
            id: "2b".to_string(),
            group_id: Vec::new(),
            ..Default::default()
        },
    ]
}

/// A start call mints a task and the reader reports that record: a fresh, legal
/// `RecommendationTaskId`, the stored start time, the non-terminal status while the clock
/// still owes the task one, and no file members at all.
#[tokio::test]
async fn a_report_generation_task_is_reported_from_what_was_stored() {
    // ONE emulator for every row, so each row's id is minted by a store that already
    // holds the earlier rows' tasks and has to be fresh. Between rows the test waits out
    // the completion span, because while a task is in progress a start call coalesces
    // onto it (the next test's subject) instead of minting.
    let router = emulator();
    let mut seen_ids: Vec<String> = Vec::new();

    for (case, request) in start_cases() {
        let started_at = Instant::now();
        let started = start_report(&router, request).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartRecommendationReportGeneration, got {} \
             with {:?}",
            started.status, started.body
        );

        let id = started.string("/id");
        assert!(
            matches_recommendation_task_id(&id),
            "case {case:?}: expected a RecommendationTaskId ([0-9a-z-:]+, max 52), got {id:?}"
        );
        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: every task gets a fresh id, but {id:?} was already minted \
             ({seen_ids:?})"
        );
        seen_ids.push(id.clone());

        let reported = report_details(&router, &id).await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: expected HTTP 200 from GetRecommendationReportDetails, got {} with \
             {:?}",
            reported.status, reported.body
        );
        assert_eq!(
            reported.string("/id"),
            id,
            "case {case:?}: the reader must report the id it was asked about, got {:?}",
            reported.body
        );
        let status = reported.string("/recommendationReportDetails/status");
        assert!(
            REPORT_STATUSES.contains(&status.as_str()),
            "case {case:?}: expected a RecommendationReportStatus value, got {status:?}"
        );
        assert!(
            at(&reported.body, "/recommendationReportDetails/startTime").is_number(),
            "case {case:?}: every task reports the start time it was stored with, got {:?}",
            at(&reported.body, "/recommendationReportDetails/startTime")
        );
        // This account named no bucket for the reports the service generates, so no report
        // FILE was written and nothing names one — on a running task and on a settled one
        // alike.
        for member in ["/s3Bucket", "/s3Keys"] {
            let pointer = format!("/recommendationReportDetails{member}");
            assert_eq!(
                at(&reported.body, &pointer),
                Value::Null,
                "case {case:?}: no report file was written, so {member} must be absent, got {:?}",
                at(&reported.body, &pointer)
            );
        }

        // The terminal status is owed by the clock: inside the span the task is still
        // running, and its completion members are absent until it is not.
        if started_at.elapsed() < COMPLETION_SPAN {
            assert_eq!(
                status, REPORT_IN_PROGRESS,
                "case {case:?}: a task read inside the completion span is still \
                 {REPORT_IN_PROGRESS}, got {status:?}"
            );
            for member in ["/completionTime", "/statusMessage"] {
                let pointer = format!("/recommendationReportDetails{member}");
                assert_eq!(
                    at(&reported.body, &pointer),
                    Value::Null,
                    "case {case:?}: a running task reports no {member}, got {:?}",
                    at(&reported.body, &pointer)
                );
            }
        }

        // Let the clock settle this task, so the next row's start call mints its own.
        std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));
    }
}

/// While one task is in progress, a start call is answered with that task's own id and
/// mints nothing — whatever parameters it carries, and whether or not they are the running
/// task's, and never with the modelled 409 `ConflictException`. Once the clock has settled
/// the task, the next start call mints a new one.
#[tokio::test]
async fn a_start_while_one_is_in_progress_coalesces_onto_it() {
    let (router, engine) = emulator_with_engine();

    let started_at = Instant::now();
    let first = start_report(&router, json!({})).await;
    assert_eq!(
        first.status, 200,
        "the first start call was refused: {:?}",
        first.body
    );
    let first_id = first.string("/id");

    // How many rows really observed the coalescing: a row the clock got ahead of is
    // skipped rather than asserted wrongly, so the table has to state it did not skip
    // everything.
    let mut observed = 0usize;
    for (case, request) in start_cases() {
        if started_at.elapsed() >= COMPLETION_SPAN {
            // The clock has already settled the task, so a further call is no longer
            // "while in progress": the row would prove nothing and is skipped rather
            // than asserted wrongly.
            break;
        }
        let again = start_report(&router, request).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected HTTP 200 (never HTTP {CONFLICT_STATUS}) from a start call \
             made while a task is in progress, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "",
            "case {case:?}: expected no {CONFLICT_EXCEPTION} for a repeat of the start, got {:?}",
            again.body
        );
        assert_eq!(
            again.string("/id"),
            first_id,
            "case {case:?}: the call must answer the id of the task already in flight \
             ({first_id:?}), got {:?}",
            again.body
        );
        assert_eq!(
            stored_task_count(&engine),
            1,
            "case {case:?}: a coalesced start call must mint nothing, but the store holds {} \
             tasks",
            stored_task_count(&engine)
        );
        // And the running task is left exactly as it was: still running, still the one
        // the first call minted.
        let running = report_details(&router, &first_id).await;
        assert_eq!(
            (
                running.status,
                running.string("/recommendationReportDetails/status")
            ),
            (200, REPORT_IN_PROGRESS.to_string()),
            "case {case:?}: the coalesced call disturbed the running task, got {:?}",
            running.body
        );
        observed += 1;
    }
    assert!(
        observed > 0,
        "no row observed the coalescing at all: expected at least one of the {} rows to be \
         asserted inside the completion span",
        start_cases().len()
    );

    // Nothing is done here but wait: no call drives the transition.
    std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));

    let settled = report_details(&router, &first_id).await;
    assert_eq!(
        settled.string("/recommendationReportDetails/status"),
        REPORT_SETTLED,
        "once the completion span has passed the task is {REPORT_SETTLED}, got {:?}",
        settled.body
    );

    let next = start_report(&router, json!({})).await;
    assert_eq!(
        next.status, 200,
        "a start call made after the running task settled is accepted, got {} with {:?}",
        next.status, next.body
    );
    let next_id = next.string("/id");
    assert_ne!(
        next_id, first_id,
        "a start call made after the running task settled must mint a new task, got {next_id:?}"
    );
    assert_eq!(
        stored_task_count(&engine),
        2,
        "the settled task and the new one are both kept, but the store holds {} tasks",
        stored_task_count(&engine)
    );
    assert_eq!(
        report_details(&router, &next_id)
            .await
            .string("/recommendationReportDetails/status"),
        REPORT_IN_PROGRESS,
        "a freshly minted task is {REPORT_IN_PROGRESS}"
    );
}

/// What the task's scope is counted from: the servers the `groupIdFilter` selects, over a
/// seeded inventory. Each row is (what it probes, the filter, how many servers are in
/// scope).
fn scope_cases() -> Vec<(&'static str, Value, usize)> {
    vec![
        ("no filter: the whole portfolio", json!({}), 4),
        (
            "an empty filter scopes nothing, so again the whole portfolio",
            json!({"groupIdFilter": []}),
            4,
        ),
        (
            "one group, which two servers came in with",
            json!({"groupIdFilter": [group(GROUP_NAMES[0], "wave-1")]}),
            2,
        ),
        (
            "two groups widen the scope",
            json!({"groupIdFilter": [
                group(GROUP_NAMES[0], "wave-1"),
                group(GROUP_NAMES[0], "wave-2"),
            ]}),
            3,
        ),
        (
            "the other GroupName, which one server came in with",
            json!({"groupIdFilter": [group(GROUP_NAMES[1], "MPA")]}),
            1,
        ),
        (
            "a group no server came in with",
            json!({"groupIdFilter": [group(GROUP_NAMES[0], "wave-9")]}),
            0,
        ),
    ]
}

/// Once the clock owes it, the task reaches its terminal status with the completion time
/// it was due and a message reporting what was in scope — and that scope is counted from
/// the inventory the filter selects, not from the request.
///
/// The WORDING is this emulator's own (no AWS rule produces it); what the row pins is the
/// status, the completion time, the published bounds of the message and the one value in
/// it that is computed from state.
#[tokio::test]
async fn a_settled_task_reports_its_terminal_status_and_its_scope() {
    for (case, request, in_scope) in scope_cases() {
        let router = emulator_with(inventory());
        let started = start_report(&router, request).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartRecommendationReportGeneration, got {} \
             with {:?}",
            started.status, started.body
        );
        let id = started.string("/id");

        // Nothing is done here but wait: no call drives the transition.
        std::thread::sleep(COMPLETION_SPAN + Duration::from_millis(200));

        let settled = report_details(&router, &id).await;
        assert_eq!(
            settled.string("/recommendationReportDetails/status"),
            REPORT_SETTLED,
            "case {case:?}: once the completion span has passed the task is {REPORT_SETTLED}, got \
             {:?}",
            settled.body
        );
        let completion = at(&settled.body, "/recommendationReportDetails/completionTime");
        let start = at(&settled.body, "/recommendationReportDetails/startTime");
        assert!(
            completion.is_number(),
            "case {case:?}: a settled task reports a completion time, got {completion:?}"
        );
        assert!(
            completion.as_f64() >= start.as_f64(),
            "case {case:?}: the completion time {completion:?} precedes the start time {start:?}"
        );

        let message = settled.string("/recommendationReportDetails/statusMessage");
        assert!(
            !message.trim().is_empty() && message.len() <= STATUS_MESSAGE_MAX,
            "case {case:?}: expected a non-blank status message within {STATUS_MESSAGE_MAX} \
             characters, got {message:?}"
        );
        assert!(
            message.contains(&in_scope.to_string()),
            "case {case:?}: {in_scope} server(s) are in scope, but the message does not report \
             that count: {message:?}"
        );
        for member in ["/s3Bucket", "/s3Keys"] {
            let pointer = format!("/recommendationReportDetails{member}");
            assert_eq!(
                at(&settled.body, &pointer),
                Value::Null,
                "case {case:?}: this account named no report data bucket, so no report file was \
                 written and {member} must be absent, got {:?}",
                at(&settled.body, &pointer)
            );
        }
    }
}

/// Ids that satisfy `RecommendationTaskId`'s rules and still name no task: each row a
/// different SHAPE of id, so nothing about the answer depends on the id itself.
const UNRESOLVABLE_IDS: &[(&str, &str)] = &[
    ("the all-zero uuid", "00000000-0000-0000-0000-000000000000"),
    (
        "a minted-looking id nothing minted",
        "recommendation-report-0000ffffffff",
    ),
    ("a single character", "z"),
    ("a colon-separated id", "report:0:never-created"),
    (
        "52 characters, at RecommendationTaskId's @length max",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ),
];

/// An id that names no task is reported as an absent resource (404
/// `ResourceNotFoundException`), never as a 200 whose `recommendationReportDetails` is
/// empty — and the refusal quotes nothing back from the request.
#[tokio::test]
async fn an_id_that_names_no_task_is_refused_as_an_absent_resource() {
    let router = emulator();
    // A task exists, so the refusals below are about the id and not about an empty store.
    let minted = start_report(&router, json!({})).await.string("/id");

    for (shape, id) in UNRESOLVABLE_IDS {
        assert_ne!(
            *id, minted,
            "case {shape:?}: this row's id is the one the store actually holds"
        );
        let answer = report_details(&router, id).await;
        assert_eq!(
            answer.status, 404,
            "case {shape:?}: expected HTTP 404 for an id naming no task, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {shape:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert!(
            !answer.string("/message").contains(id),
            "case {shape:?}: the refusal quoted the caller's id back at it: {:?}",
            answer.string("/message")
        );
        assert_eq!(
            at(&answer.body, "/recommendationReportDetails"),
            Value::Null,
            "case {shape:?}: a refusal carries no report details, got {:?}",
            answer.body
        );
    }
}

/// A start request carrying an enum value the model does not declare, or a
/// `groupIdFilter` past the bound this operation accepts, is refused with 400
/// `ValidationException` and mints nothing.
#[tokio::test]
async fn an_undeclared_enum_value_or_an_oversized_filter_is_refused() {
    let (router, engine) = emulator_with_engine();

    let mut refused: Vec<(String, Value)> = vec![
        (
            "an outputFormat OutputFormat does not declare".to_string(),
            json!({"outputFormat": "Pdf"}),
        ),
        (
            "the right format in the wrong case".to_string(),
            json!({"outputFormat": "excel"}),
        ),
        (
            "a groupIdFilter name GroupName does not declare".to_string(),
            json!({"groupIdFilter": [{"name": "WaveId", "value": "wave-1"}]}),
        ),
        (
            "one legal group and one undeclared one".to_string(),
            json!({"groupIdFilter": [
                group(GROUP_NAMES[0], "wave-1"),
                {"name": "WaveId", "value": "wave-2"},
            ]}),
        ),
    ];
    // One entry past the bound, and far past it: `GroupIds` declares no `@length`, so the
    // bound is the emulator's own and is enforced by the handler.
    for over in [MAX_GROUP_FILTER_ENTRIES + 1, MAX_GROUP_FILTER_ENTRIES * 3] {
        refused.push((
            format!("a groupIdFilter of {over} entries, past the accepted {MAX_GROUP_FILTER_ENTRIES}"),
            json!({"groupIdFilter": filter_of(over)}),
        ));
    }

    for (case, request) in refused {
        let answer = start_report(&router, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            stored_task_count(&engine),
            0,
            "case {case:?}: a refused start call must mint nothing, but the store holds {} tasks",
            stored_task_count(&engine)
        );
    }

    // AT the bound the filter is accepted: the refusal is for passing it, not for
    // filtering at all.
    let at_bound = start_report(
        &router,
        json!({"groupIdFilter": filter_of(MAX_GROUP_FILTER_ENTRIES)}),
    )
    .await;
    assert_eq!(
        at_bound.status, 200,
        "a groupIdFilter of exactly {MAX_GROUP_FILTER_ENTRIES} entries is accepted, got {} with \
         {:?}",
        at_bound.status, at_bound.body
    );
}

/// A `groupIdFilter` of `entries` legal groups.
fn filter_of(entries: usize) -> Value {
    Value::Array(
        (0..entries)
            .map(|index| group(GROUP_NAMES[index % GROUP_NAMES.len()], &format!("wave-{index}")))
            .collect(),
    )
}

/// Once the store holds the quota of tasks, a start call is refused with the operation's
/// modelled `ThrottlingException` (HTTP 429) instead of minting another record — the
/// bound that stops the state growing without end.
///
/// The quota is reached by seeding settled records straight into the store (the same map
/// the handler writes), because reaching it through the API would mean waiting out one
/// completion span per task.
#[tokio::test]
async fn a_start_call_past_the_quota_is_refused() {
    let (router, engine) = emulator_with_engine();
    let settled_at = SystemTime::now();
    {
        let state = engine
            .state::<ComAmazonawsMigrationhubstrategyAwsmigrationhubstrategyrecommendationState>();
        let mut store = state.lock();
        let portfolio = store.region_entry(TEST_REGION);
        for index in 0..REPORT_QUOTA {
            let id = format!("seeded-report-{index:012x}");
            portfolio.recommendation_reports.insert(
                id.clone(),
                RecommendationReport {
                    id,
                    status: REPORT_SETTLED.to_string(),
                    status_message: None,
                    output_format: OUTPUT_FORMATS[0].to_string(),
                    servers_covered: 0,
                    start_time: settled_at,
                    completion_time: Some(settled_at),
                    // Settled: nothing is owed, so the call below is not refused for a
                    // task still running.
                    completion_due: None,
                    // Seeded settled, and this account named no report data bucket, so
                    // there is no report file for the task to name.
                    s3_bucket: None,
                    s3_keys: Vec::new(),
                    no_file_reason: None,
                },
            );
        }
    }

    let answer = start_report(&router, json!({})).await;
    assert_eq!(
        answer.status, 429,
        "expected HTTP 429 once the store holds the quota of {REPORT_QUOTA} tasks, got {} with \
         {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ThrottlingException",
        "expected ThrottlingException at the quota, got {:?}",
        answer.body
    );
    assert_eq!(
        stored_task_count(&engine),
        REPORT_QUOTA,
        "a refused start call must mint nothing, but the store holds {} tasks",
        stored_task_count(&engine)
    );
}
