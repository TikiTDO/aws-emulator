// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `ListTrainingJobs`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.ListTrainingJobs`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets.
//!
//! The training jobs themselves are put into the shared store directly, through the same
//! `crate::state` records `CreateTrainingJob` writes, so each fixture row's instants,
//! statuses, warm pool and training plan are exactly what the table declares rather than
//! whatever a create would mint from the wall clock — which is what the listing's order,
//! filters and projection are asserted over. Seeding is therefore the fixture, and only
//! the listing is under test, which is exactly what the shared harness's
//! `pub(crate) engine` is for, so this suite reuses
//! [`super::inference_deployment_lifecycle::Emulator`] rather than growing another copy
//! of the dispatch path.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! documented sort field and order, every documented filter, the projection of each stored
//! member into a `TrainingJobSummary`, the paging walk at every page size up to past the
//! end of the result set, the documented `StatusEquals`/`MaxResults` interaction, and the
//! refusals the modelled constraints require.

use std::time::Duration;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::ops::types::{
    CreateTrainingJobInput, WarmPoolStatus,
};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    ComAmazonawsSagemakerSagemakerState, TrainingJob, MAX_PAGE_SIZE,
};

use super::inference_deployment_lifecycle::{error_code, Emulator, ACCOUNT};

/// The region these tests sign for, so a seeded ARN and a minted one agree — the one
/// [`Emulator::call`] signs for.
const REGION: &str = "us-east-1";

/// The instant the fixture's timestamps hang off, so every bound in a table is exact
/// rather than relative to the test's wall clock.
const EPOCH_BASE_SECS: u64 = 1_700_000_000;

/// `TrainingJobStatus`'s members that `TrainingJobSummary`'s `TrainingEndTime` doc calls
/// terminal ("set only if the training job has one of the terminal statuses
/// (`Completed`, `Failed`, or `Stopped`)").
const TERMINAL_STATUSES: [&str; 3] = ["Completed", "Failed", "Stopped"];

/// Put a training job in the shared store, under the `TrainingJobName` it is keyed by.
///
/// The store is reached through the harness's `engine` so each row's instants, statuses
/// and optional members are exactly what the table declares, which is what the listing's
/// order, filters and projection are asserted over.
fn seed(emulator: &Emulator, job: TrainingJob) {
    emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>()
        .lock()
        .training_jobs
        .insert(job.name.clone(), job);
}

/// The names the store holds, so a read can be shown not to change it.
fn stored_jobs(emulator: &Emulator) -> Vec<String> {
    emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>()
        .lock()
        .training_jobs
        .keys()
        .cloned()
        .collect()
}

/// `ListTrainingJobs` with `body`, requiring a 200.
async fn list(emulator: &Emulator, body: &Value) -> Value {
    emulator.ok("ListTrainingJobs", body).await
}

/// `EPOCH_BASE_SECS + offset` as the epoch-seconds number `awsJson1_1` carries a
/// `Timestamp` as.
fn epoch_secs(offset: u64) -> i64 {
    i64::try_from(EPOCH_BASE_SECS + offset).expect("the fixture's instants fit an i64")
}

/// `EPOCH_BASE_SECS + offset` as a stored instant.
fn at(offset: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(EPOCH_BASE_SECS + offset)
}

/// A `training-job` ARN of the form this service mints
/// (`arn:aws:sagemaker:<region>:<account>:training-job/<name>`).
fn training_job_arn(name: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:training-job/{name}")
}

/// A `TrainingPlanArn`: `len 50..2048`, pattern
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:training-plan/`
/// (smithy_query shape=TrainingPlanArn).
fn training_plan_arn(name: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:training-plan/{name}")
}

/// The `SecondaryStatus` the fixture stores beside each `TrainingJobStatus`. Every value
/// is a member of the modelled `SecondaryStatus` enum (smithy_query shape=SecondaryStatus).
fn secondary_status_for(status: &str) -> &'static str {
    match status {
        "InProgress" => "Training",
        "Completed" => "Completed",
        "Failed" => "Failed",
        "Stopping" => "Stopping",
        "Stopped" => "Stopped",
        _ => "Pending",
    }
}

/// A stored training job: `created` and `modified` are offsets from [`EPOCH_BASE_SECS`],
/// and `TrainingEndTime` is set exactly for the terminal statuses.
fn job(name: &str, status: &str, created: u64, modified: u64) -> TrainingJob {
    TrainingJob {
        name: name.to_string(),
        arn: training_job_arn(name),
        status: status.to_string(),
        secondary_status: Some(secondary_status_for(status).to_string()),
        creation_time: at(created),
        last_modified_time: at(modified),
        training_end_time: TERMINAL_STATUSES
            .contains(&status)
            .then(|| at(modified)),
        warm_pool_status: None,
        training_plan_arn: None,
        failure_reason: None,
        // A row seeded in a terminal status is already where the clock would leave it, so
        // it carries no recorded run, and the listing reports neither it nor the artifact,
        // the start instant and the metrics a run reports.
        run: None,
        training_start_time: None,
        model_artifacts: None,
        final_metrics: Vec::new(),
        ends_at: at(modified),
        // The stage the row is seeded in, which is the one its `SecondaryStatus` names: a
        // row is stored where the clock has already left it, so nothing more is due.
        phases: vec![service_com_amazonaws_sagemaker_sagemaker::state::TrainingPhase {
            status: secondary_status_for(status).to_string(),
            starts_at: at(created),
        }],
        entered: 1,
        tuning_job_arn: None,
        request: CreateTrainingJobInput::default(),
        tags: Vec::new(),
    }
}

/// A warm pool in `status` (a `WarmPoolResourceStatus` member).
fn warm_pool(status: &str) -> WarmPoolStatus {
    WarmPoolStatus {
        status: status.to_string(),
        resource_retained_billable_time_in_seconds: Some(60),
        reused_by_job: None,
    }
}

/// Four training jobs whose rows differ along name, status, creation time, last-modified
/// time (ordered the OPPOSITE way to creation time, so the two bounds cannot be confused),
/// warm-pool presence and status, and training-plan ARN.
fn seed_fixture(emulator: &Emulator) {
    let mut alpha = job("alpha-one", "InProgress", 10, 40);
    alpha.warm_pool_status = Some(warm_pool("InUse"));

    let mut beta = job("beta-two", "Completed", 20, 30);
    beta.warm_pool_status = Some(warm_pool("Available"));
    beta.training_plan_arn = Some(training_plan_arn("reserved"));

    let mut three = job("demo-three", "Failed", 30, 20);
    three.training_plan_arn = Some(training_plan_arn("reserved"));

    let mut four = job("demo-four", "Stopped", 40, 10);
    four.warm_pool_status = Some(warm_pool("Terminated"));

    for record in [alpha, beta, three, four] {
        seed(emulator, record);
    }
}

/// One row of the projection table: a stored fixture job, and what its summary must
/// carry. `created` and `modified` are offsets from [`EPOCH_BASE_SECS`]; `warm_pool` is
/// the warm pool's status where the job has one, and `planned` says whether it names the
/// fixture's training plan.
struct ExpectedSummary {
    name: &'static str,
    status: &'static str,
    secondary: &'static str,
    created: u64,
    modified: u64,
    warm_pool: Option<&'static str>,
    planned: bool,
}

/// The `TrainingJobName`s a response lists, in the order it listed them.
fn listed_names(listed: &Value) -> Vec<String> {
    summaries(listed)
        .iter()
        .map(|summary| {
            summary
                .get("TrainingJobName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

fn summaries(listed: &Value) -> Vec<Value> {
    listed
        .get("TrainingJobSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("TrainingJobSummaries is a required output member: {listed}"))
}

fn next_token(listed: &Value) -> Option<String> {
    listed
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// RULE: with nothing stored, the listing answers 200 with an empty
/// `TrainingJobSummaries` and no `NextToken` — never a 501, and never a token to follow.
#[tokio::test]
async fn an_empty_store_lists_no_training_jobs() {
    let emulator = Emulator::new();

    // Rows: the bare call, and one carrying each filter — an absent resource is an empty
    // page for a listing whatever the request selected on.
    let cases = [
        ("no-members", json!({})),
        ("name-contains", json!({ "NameContains": "anything" })),
        ("status-equals", json!({ "StatusEquals": "InProgress" })),
        (
            "max-results",
            json!({ "MaxResults": 100, "SortBy": "Name", "SortOrder": "Descending" }),
        ),
    ];

    for (case, body) in cases {
        let listed = list(&emulator, &body).await;
        assert_eq!(
            listed_names(&listed),
            Vec::<String>::new(),
            "case {case}: an empty store must list nothing, got {listed:?}"
        );
        assert_eq!(
            next_token(&listed),
            None,
            "case {case}: an empty page must carry no NextToken, got {listed:?}"
        );
    }
}

/// RULE: the listing orders the stored jobs by `SortBy` in `SortOrder`, defaulting to
/// `CreationTime` / `Ascending` as those members' model docs state, and breaks a tie on
/// `TrainingJobName` ascending in both directions.
#[tokio::test]
async fn the_listing_honours_every_documented_sort_field_and_order() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    // `SortBy` is `Name | CreationTime | Status` and `SortOrder` is
    // `Ascending | Descending` (smithy_query operation=ListTrainingJobs): the rows below
    // are every combination, plus the two defaults.
    let by_creation = ["alpha-one", "beta-two", "demo-three", "demo-four"];
    let by_name = ["alpha-one", "beta-two", "demo-four", "demo-three"];
    // `Completed` < `Failed` < `InProgress` < `Stopped`.
    let by_status = ["beta-two", "demo-three", "alpha-one", "demo-four"];

    let reversed = |order: [&'static str; 4]| {
        let mut rows = order.to_vec();
        rows.reverse();
        rows
    };

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("defaults", json!({}), by_creation.to_vec()),
        (
            "creation-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            by_creation.to_vec(),
        ),
        (
            "creation-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            reversed(by_creation),
        ),
        (
            "name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            by_name.to_vec(),
        ),
        (
            "name-descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            reversed(by_name),
        ),
        (
            "status-ascending",
            json!({ "SortBy": "Status", "SortOrder": "Ascending" }),
            by_status.to_vec(),
        ),
        (
            "status-descending",
            json!({ "SortBy": "Status", "SortOrder": "Descending" }),
            reversed(by_status),
        ),
        (
            "sort-order-only",
            json!({ "SortOrder": "Descending" }),
            reversed(by_creation),
        ),
        ("sort-by-only", json!({ "SortBy": "Name" }), by_name.to_vec()),
    ];

    for (case, body, expected) in cases {
        let listed = list(&emulator, &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListTrainingJobs order, got {listed:?}"
        );
    }
}

/// RULE: jobs created in the same instant are ordered by `TrainingJobName` ascending
/// whichever way the sort key runs, so a page boundary is stable.
#[tokio::test]
async fn jobs_sharing_a_sort_key_are_tie_broken_by_name_in_both_orders() {
    let emulator = Emulator::new();
    // Three jobs with one creation instant and one status: only the tie-break orders them.
    for name in ["c-job", "a-job", "b-job"] {
        seed(&emulator, job(name, "InProgress", 10, 10));
    }

    for (case, body) in [
        (
            "creation-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        ),
        (
            "creation-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
        ),
        (
            "status-descending",
            json!({ "SortBy": "Status", "SortOrder": "Descending" }),
        ),
    ] {
        let listed = list(&emulator, &body).await;
        assert_eq!(
            listed_names(&listed),
            vec!["a-job", "b-job", "c-job"],
            "case {case}: a shared sort key must be tie-broken by name ascending, got {listed:?}"
        );
    }
}

/// RULE: each filter selects on the job's stored value — `NameContains` on a substring of
/// the name, the four time bounds inclusively on the stored creation and last-modified
/// instants, `StatusEquals` on the status, `WarmPoolStatusEquals` on the warm pool's
/// status (so a job without a warm pool is excluded), and `TrainingPlanArnEquals` on the
/// whole training-plan ARN. An omitted filter selects every job.
#[tokio::test]
async fn every_documented_filter_selects_on_the_stored_value() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        // NameContains: a substring anywhere in the name, not a prefix.
        (
            "name-contains-prefix",
            json!({ "NameContains": "demo" }),
            vec!["demo-three", "demo-four"],
        ),
        (
            "name-contains-infix",
            json!({ "NameContains": "two" }),
            vec!["beta-two"],
        ),
        (
            "name-contains-whole-name",
            json!({ "NameContains": "alpha-one" }),
            vec!["alpha-one"],
        ),
        (
            "name-contains-no-match",
            json!({ "NameContains": "gamma" }),
            vec![],
        ),
        // CreationTime bounds: both inclusive, so a bound copied out of a prior
        // response still selects the job it came from.
        (
            "creation-after-inclusive",
            json!({ "CreationTimeAfter": epoch_secs(20) }),
            vec!["beta-two", "demo-three", "demo-four"],
        ),
        (
            "creation-before-inclusive",
            json!({ "CreationTimeBefore": epoch_secs(20) }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "creation-window",
            json!({
                "CreationTimeAfter": epoch_secs(20),
                "CreationTimeBefore": epoch_secs(30),
            }),
            vec!["beta-two", "demo-three"],
        ),
        (
            "creation-window-empty",
            json!({
                "CreationTimeAfter": epoch_secs(41),
                "CreationTimeBefore": epoch_secs(100),
            }),
            vec![],
        ),
        // LastModifiedTime bounds: the fixture's modification order is the reverse of
        // its creation order, so a bound cannot select the right rows by accident.
        (
            "last-modified-after",
            json!({ "LastModifiedTimeAfter": epoch_secs(30) }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "last-modified-before",
            json!({ "LastModifiedTimeBefore": epoch_secs(20) }),
            vec!["demo-three", "demo-four"],
        ),
        (
            "last-modified-window",
            json!({
                "LastModifiedTimeAfter": epoch_secs(20),
                "LastModifiedTimeBefore": epoch_secs(30),
            }),
            vec!["beta-two", "demo-three"],
        ),
        // StatusEquals: every member of TrainingJobStatus, present in the fixture or not.
        (
            "status-in-progress",
            json!({ "StatusEquals": "InProgress" }),
            vec!["alpha-one"],
        ),
        (
            "status-completed",
            json!({ "StatusEquals": "Completed" }),
            vec!["beta-two"],
        ),
        (
            "status-failed",
            json!({ "StatusEquals": "Failed" }),
            vec!["demo-three"],
        ),
        (
            "status-stopped",
            json!({ "StatusEquals": "Stopped" }),
            vec!["demo-four"],
        ),
        (
            "status-stopping-absent",
            json!({ "StatusEquals": "Stopping" }),
            vec![],
        ),
        (
            "status-deleting-absent",
            json!({ "StatusEquals": "Deleting" }),
            vec![],
        ),
        // WarmPoolStatusEquals: every member of WarmPoolResourceStatus.
        (
            "warm-pool-in-use",
            json!({ "WarmPoolStatusEquals": "InUse" }),
            vec!["alpha-one"],
        ),
        (
            "warm-pool-available",
            json!({ "WarmPoolStatusEquals": "Available" }),
            vec!["beta-two"],
        ),
        (
            "warm-pool-terminated",
            json!({ "WarmPoolStatusEquals": "Terminated" }),
            vec!["demo-four"],
        ),
        (
            "warm-pool-reused-absent",
            json!({ "WarmPoolStatusEquals": "Reused" }),
            vec![],
        ),
        // TrainingPlanArnEquals: the whole ARN, not a prefix of it.
        (
            "training-plan-matches",
            json!({ "TrainingPlanArnEquals": training_plan_arn("reserved") }),
            vec!["beta-two", "demo-three"],
        ),
        (
            "training-plan-other",
            json!({ "TrainingPlanArnEquals": training_plan_arn("other-plan") }),
            vec![],
        ),
        // Filters compose: each one narrows what the others left.
        (
            "name-and-status",
            json!({ "NameContains": "demo", "StatusEquals": "Failed" }),
            vec!["demo-three"],
        ),
        (
            "name-and-creation-window",
            json!({ "NameContains": "demo", "CreationTimeBefore": epoch_secs(35) }),
            vec!["demo-three"],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = list(&emulator, &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListTrainingJobs selection, got {listed:?}"
        );
    }
}

/// RULE: each summary replays the stored job — its name, ARN, creation and last-modified
/// instants, status and secondary status, its warm pool and training plan where it has
/// them, and a `TrainingEndTime` exactly where the status is terminal.
#[tokio::test]
async fn each_summary_replays_the_stored_training_job() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    let expected = vec![
        ExpectedSummary {
            name: "alpha-one",
            status: "InProgress",
            secondary: "Training",
            created: 10,
            modified: 40,
            warm_pool: Some("InUse"),
            planned: false,
        },
        ExpectedSummary {
            name: "beta-two",
            status: "Completed",
            secondary: "Completed",
            created: 20,
            modified: 30,
            warm_pool: Some("Available"),
            planned: true,
        },
        ExpectedSummary {
            name: "demo-three",
            status: "Failed",
            secondary: "Failed",
            created: 30,
            modified: 20,
            warm_pool: None,
            planned: true,
        },
        ExpectedSummary {
            name: "demo-four",
            status: "Stopped",
            secondary: "Stopped",
            created: 40,
            modified: 10,
            warm_pool: Some("Terminated"),
            planned: false,
        },
    ];

    let listed = list(
        &emulator,
        &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
    )
    .await;
    let summaries = summaries(&listed);
    assert_eq!(
        summaries.len(),
        expected.len(),
        "the fixture's jobs must all be listed, got {listed:?}"
    );

    for (summary, row) in summaries.iter().zip(expected) {
        let ExpectedSummary {
            name,
            status,
            secondary,
            created,
            modified,
            warm_pool: pool,
            planned,
        } = row;
        let case = name;
        assert_eq!(
            summary.get("TrainingJobName").and_then(Value::as_str),
            Some(name),
            "case {case}: TrainingJobName, got {summary:?}"
        );
        assert_eq!(
            summary.get("TrainingJobArn").and_then(Value::as_str),
            Some(training_job_arn(name).as_str()),
            "case {case}: TrainingJobArn, got {summary:?}"
        );
        assert_eq!(
            summary.get("CreationTime").and_then(Value::as_i64),
            Some(epoch_secs(created)),
            "case {case}: CreationTime, got {summary:?}"
        );
        assert_eq!(
            summary.get("LastModifiedTime").and_then(Value::as_i64),
            Some(epoch_secs(modified)),
            "case {case}: LastModifiedTime, got {summary:?}"
        );
        assert_eq!(
            summary.get("TrainingJobStatus").and_then(Value::as_str),
            Some(status),
            "case {case}: TrainingJobStatus, got {summary:?}"
        );
        assert_eq!(
            summary.get("SecondaryStatus").and_then(Value::as_str),
            Some(secondary),
            "case {case}: SecondaryStatus, got {summary:?}"
        );
        // `TrainingEndTime` is "set only if the training job has one of the terminal
        // statuses (Completed, Failed, or Stopped)".
        let terminal = TERMINAL_STATUSES.contains(&status);
        assert_eq!(
            summary.get("TrainingEndTime").and_then(Value::as_i64),
            terminal.then(|| epoch_secs(modified)),
            "case {case}: TrainingEndTime must be present exactly for a terminal status, \
             got {summary:?}"
        );
        assert_eq!(
            summary
                .get("WarmPoolStatus")
                .and_then(|status| status.get("Status"))
                .and_then(Value::as_str),
            pool,
            "case {case}: WarmPoolStatus.Status, got {summary:?}"
        );
        assert_eq!(
            summary.get("TrainingPlanArn").and_then(Value::as_str),
            planned.then(|| training_plan_arn("reserved")).as_deref(),
            "case {case}: TrainingPlanArn, got {summary:?}"
        );
    }
}

/// RULE: paging with `MaxResults` walks the whole ordered result exactly once — each page
/// holds at most `MaxResults` entries, a `NextToken` is present only while more remain,
/// and feeding it back resumes immediately after the previous page.
#[tokio::test]
async fn paging_walks_the_whole_ordered_result_exactly_once() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);
    let ordered = vec!["alpha-one", "beta-two", "demo-four", "demo-three"];

    // Rows: every page size from one entry up to past the end of the result set.
    // `MaxResults` is `range 1..100` (smithy_query operation=ListTrainingJobs), so 1 and
    // 100 are the published bounds.
    for page_size in [1_i64, 2, 3, 4, 5, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= 10,
                "case {case}: paging must terminate, saw {seen:?}"
            );
            let mut body = json!({
                "SortBy": "Name",
                "SortOrder": "Ascending",
                "MaxResults": page_size,
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = list(&emulator, &body).await;
            let names = listed_names(&listed);
            assert!(
                names.len() as i64 <= page_size,
                "case {case}: a page must hold at most {page_size} entries, got {names:?}"
            );
            seen.extend(names);
            token = next_token(&listed);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, ordered,
            "case {case}: the walk must cover the ordered result exactly once"
        );
    }

    // A token that names no boundary of this result set is refused rather than silently
    // restarting the listing.
    let (status, parsed) = emulator
        .call("ListTrainingJobs", &json!({ "NextToken": "not-a-token" }))
        .await;
    assert_eq!(
        status, 400,
        "case bogus-NextToken: expected HTTP 400, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case bogus-NextToken: error code, got {parsed:?}"
    );
}

/// RULE: the page is capped server-side, so no single call serialises the whole store
/// however large `MaxResults` is or whether the caller sends it at all — and the resume
/// token still walks the full set.
#[tokio::test]
async fn the_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    let ceiling = usize::try_from(MAX_PAGE_SIZE).expect("the page ceiling is positive");
    let stored = ceiling + 7;

    let emulator = Emulator::new();
    for index in 0..stored {
        seed(&emulator, job(
            &format!("job-{index:03}"),
            "InProgress",
            index as u64,
            index as u64,
        ));
    }

    for (case, body, expected_page) in [
        ("no-max-results", json!({}), ceiling),
        ("max-results-at-the-bound", json!({ "MaxResults": 100 }), ceiling),
        ("max-results-below-the-ceiling", json!({ "MaxResults": 7 }), 7),
    ] {
        let mut request = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        for (key, value) in body.as_object().expect("a JSON object of extra members") {
            request[key] = value.clone();
        }
        let listed = list(&emulator, &request).await;
        assert_eq!(
            listed_names(&listed).len(),
            expected_page,
            "case {case}: page size, got {} entries",
            listed_names(&listed).len()
        );
        assert!(
            next_token(&listed).is_some(),
            "case {case}: entries remain, so a NextToken must be present, got {listed:?}"
        );
    }

    // The capped walk still reaches every stored job exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = list(&emulator, &body).await;
        let names = listed_names(&listed);
        assert!(
            names.len() <= ceiling,
            "case capped-walk: a page must never exceed the ceiling, got {} entries",
            names.len()
        );
        seen.extend(names);
        token = next_token(&listed);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen.len(),
        stored,
        "case capped-walk: the walk must cover every stored job exactly once, got {} entries",
        seen.len()
    );
}

/// RULE: `StatusEquals` is applied to the RETRIEVED page, not to the result set — "the
/// MaxResults number of training jobs are first retrieved ignoring the StatusEquals
/// parameter and then they are filtered by the StatusEquals parameter, which is returned
/// as a response" (ListTrainingJobs' model doc). So a page can come back shorter than
/// `MaxResults`, or empty, while still carrying a `NextToken`, and the walk ends up with
/// every matching job exactly once.
#[tokio::test]
async fn status_equals_filters_the_retrieved_page_and_the_walk_still_finds_every_match() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    // Retrieval order by creation ascending is alpha-one, beta-two, demo-three,
    // demo-four, so a one-entry page retrieves alpha-one (InProgress) first: filtering it
    // by `Completed` reports nothing, yet the listing is not finished.
    let first = list(
        &emulator,
        &json!({
            "MaxResults": 1,
            "StatusEquals": "Completed",
            "SortBy": "CreationTime",
            "SortOrder": "Ascending",
        }),
    )
    .await;
    assert_eq!(
        listed_names(&first),
        Vec::<String>::new(),
        "case retrieved-page-filtered-empty: the retrieved job does not match the status, \
         got {first:?}"
    );
    let token = next_token(&first).unwrap_or_else(|| {
        panic!("case retrieved-page-filtered-empty: jobs remain, so a NextToken must be present, got {first:?}")
    });

    let second = list(
        &emulator,
        &json!({
            "MaxResults": 1,
            "StatusEquals": "Completed",
            "SortBy": "CreationTime",
            "SortOrder": "Ascending",
            "NextToken": token,
        }),
    )
    .await;
    assert_eq!(
        listed_names(&second),
        vec!["beta-two"],
        "case resumed-page: the next retrieved job matches the status, got {second:?}"
    );

    // Rows: each page size, walked to the end. However the window is cut, the walk
    // reports every job of that status exactly once.
    for (page_size, status, expected) in [
        (1_i64, "Completed", vec!["beta-two"]),
        (2, "Completed", vec!["beta-two"]),
        (3, "Failed", vec!["demo-three"]),
        (4, "Stopped", vec!["demo-four"]),
        (1, "Stopping", vec![]),
    ] {
        let case = format!("walk MaxResults={page_size} StatusEquals={status}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({
                "MaxResults": page_size,
                "StatusEquals": status,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = list(&emulator, &body).await;
            seen.extend(listed_names(&listed));
            token = next_token(&listed);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: the walk must report every job of that status exactly once"
        );
    }
}

/// RULE: listing is a report — it changes nothing, so a second identical call answers
/// identically and the store still holds exactly what it held.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);
    let before = stored_jobs(&emulator);

    let cases = [
        ("no-members", json!({})),
        ("max-results", json!({ "MaxResults": 2 })),
        (
            "filtered",
            json!({ "StatusEquals": "Completed", "SortBy": "Name" }),
        ),
    ];

    for (case, body) in cases {
        let first = list(&emulator, &body).await;
        let second = list(&emulator, &body).await;
        assert_eq!(
            first, second,
            "case {case}: a repeated read must answer identically, got {first:?} then {second:?}"
        );
        assert_eq!(
            stored_jobs(&emulator),
            before,
            "case {case}: a read must not change the store"
        );
    }
}

/// RULE: a request outside a member's modelled constraint, or naming a value its enum
/// does not hold, is refused with HTTP 400 — never a 200 listing something, and never a
/// 501.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    // Rows: one past each published bound this operation's members state —
    // `MaxResults` range 1..100, `NameContains` len 0..63 with pattern
    // `^[a-zA-Z0-9\-]+$`, `TrainingPlanArn` len 50..2048 — plus a value outside each
    // sort enum.
    let cases: Vec<(&str, Value)> = vec![
        ("max-results-below-1", json!({ "MaxResults": 0 })),
        ("max-results-above-100", json!({ "MaxResults": 101 })),
        (
            "name-contains-past-63",
            json!({ "NameContains": "a".repeat(64) }),
        ),
        (
            "name-contains-outside-pattern",
            json!({ "NameContains": "bad_name" }),
        ),
        (
            "training-plan-arn-too-short",
            json!({ "TrainingPlanArnEquals": "arn:aws:sagemaker:::training-plan/x" }),
        ),
        ("sort-by-unknown", json!({ "SortBy": "EndTime" })),
        ("sort-order-wrong-case", json!({ "SortOrder": "ascending" })),
        // A value outside each status enum: `TrainingJobStatus`
        // (Completed|Deleting|Failed|InProgress|Stopped|Stopping) and
        // `WarmPoolResourceStatus` (Available|InUse|Reused|Terminated).
        ("status-equals-unknown", json!({ "StatusEquals": "Pending" })),
        (
            "status-equals-wrong-case",
            json!({ "StatusEquals": "inprogress" }),
        ),
        (
            "warm-pool-status-unknown",
            json!({ "WarmPoolStatusEquals": "Warm" }),
        ),
    ];

    for (case, body) in cases {
        let (status, parsed) = emulator.call("ListTrainingJobs", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );
    }

    // A refused request changed nothing: the fixture still lists in full.
    let listed = list(&emulator, &json!({})).await;
    assert_eq!(
        listed_names(&listed).len(),
        4,
        "a refused request must leave the store untouched, got {listed:?}"
    );
}
