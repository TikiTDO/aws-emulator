// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListJobExecutionsForThing`: the control plane lists the
//! job executions ONE device has, computed from the SHARED job-execution store
//! (`service_common::iot_job_executions`) the IoT Jobs data plane serves.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! `GET /things/{thingName}/jobs` signed for the `iot` signing name — after creating the
//! jobs through `PUT /jobs/{jobId}`, so what is listed is what a writer persisted.
//!
//! The rules under test:
//!
//! * the listing reports one entry per execution the store holds for that device, each the
//!   stored record projected onto `JobExecutionSummaryForThing` — `jobId` plus
//!   `jobExecutionSummary`'s `status`, `queuedAt`, `startedAt`, `lastUpdatedAt`,
//!   `executionNumber` and `retryAttempt` — in the store's enqueue order, and reading it
//!   does not change the records;
//! * `status` and `jobId` each narrow the listing to the executions they select, and a
//!   device with no matching execution is an EMPTY list, not a refusal;
//! * `maxResults` and `nextToken` walk that one order, every execution exactly once, and a
//!   `nextToken` that is not a token of THIS listing is refused 400
//!   `InvalidRequestException`;
//! * a `jobId` naming no job of the account is refused 404 `ResourceNotFoundException`
//!   with the jobs plane's own sentence;
//! * an execution whose armed timer the clock has already passed is listed `TIMED_OUT` —
//!   the transition that was due at the deadline — while a twin whose timer is still in
//!   the future is untouched, and a COMMAND execution the same store holds is not listed
//!   as a job execution.
//!
//! Each table spans the class a rule generalises over — how many devices a job targets,
//! how many jobs one device has, every `JobExecutionStatus` an execution can be read in,
//! each published `maxResults` bound and one past it — rather than one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use service_common::iot_job_executions::{
    now_epoch_seconds, JobExecutionRecord, JobExecutionStore, STATUS_IN_PROGRESS, STATUS_QUEUED,
    STATUS_REJECTED, STATUS_TIMED_OUT,
};
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";

/// The values `JobExecutionStatus` admits
/// (`smithy_query(query="shape",shape="JobExecutionStatus")`), which is the catalog the
/// `status` filter is driven over.
const JOB_EXECUTION_STATUSES: &[&str] = &[
    "QUEUED",
    "IN_PROGRESS",
    "SUCCEEDED",
    "FAILED",
    "TIMED_OUT",
    "REJECTED",
    "REMOVED",
    "CANCELED",
];

/// The largest page `LaserMaxResults` admits (`range 1..250`).
const MAX_RESULTS_CEILING: i32 = 250;

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name:
/// the scope is how the router picks the claiming service.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The listed entries, or an empty slice when the member is absent.
    fn entries(&self) -> &[Value] {
        self.body["executionSummaries"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
    }

    /// The listed `jobId`s, in the order they were reported.
    fn job_ids(&self) -> Vec<String> {
        self.entries()
            .iter()
            .map(|entry| entry["jobId"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// One listed entry's `jobExecutionSummary` member, or `Value::Null`.
    fn summary_member(&self, index: usize, name: &str) -> Value {
        self.entries()
            .get(index)
            .map(|entry| entry["jobExecutionSummary"][name].clone())
            .unwrap_or(Value::Null)
    }

    fn next_token(&self) -> Option<String> {
        self.body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it. The
/// engine handle is kept so a case can seed and read the SHARED job-execution store the
/// control plane writes into and the data plane serves.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    async fn create_thing(&self, thing_name: &str) -> Reply {
        self.call("POST", &format!("/things/{thing_name}"), Some(&json!({})))
            .await
    }

    /// `CreateJob` targeting every device in `things`, which is what creates one `QUEUED`
    /// execution per device.
    async fn create_job(&self, job_id: &str, things: &[&str]) -> Reply {
        let request = json!({
            "targets": things.iter().map(|thing| target_arn(thing)).collect::<Vec<String>>(),
            "document": format!(r#"{{"operation":"reboot","job":"{job_id}"}}"#),
        });
        let created = self
            .call("PUT", &format!("/jobs/{job_id}"), Some(&request))
            .await;
        assert_eq!(
            created.status, 200,
            "creating job {job_id:?} must answer 200, got {} {:?}",
            created.status, created.body
        );
        created
    }

    /// `ListJobExecutionsForThing` for one device, with `query` appended to the URI
    /// (where `status`, `jobId`, `maxResults` and `nextToken` are bound).
    async fn list_with_query(&self, thing_name: &str, query: &str) -> Reply {
        self.call("GET", &format!("/things/{thing_name}/jobs{query}"), None)
            .await
    }

    async fn list(&self, thing_name: &str) -> Reply {
        self.list_with_query(thing_name, "").await
    }

    /// `DescribeJobExecution` for a (thing, job) pair: the single-execution reader whose
    /// answer a listed entry must agree with.
    async fn describe(&self, thing_name: &str, job_id: &str) -> Reply {
        self.call("GET", &format!("/things/{thing_name}/jobs/{job_id}"), None)
            .await
    }

    /// Mutate the stored execution of (thing, job) in the SHARED store, so a case can put
    /// a record in a state only the device-facing plane reaches (a status, a start
    /// instant, an armed timer) and then list it back through this service.
    fn with_stored_execution<F>(&self, thing_name: &str, job_id: &str, mutate: F)
    where
        F: FnOnce(&mut JobExecutionRecord),
    {
        let store = self.engine.state::<JobExecutionStore>();
        let mut store = store.lock();
        let index = store
            .execution_index(thing_name, job_id, None)
            .unwrap_or_else(|| panic!("{thing_name} must hold an execution of {job_id}"));
        mutate(
            store
                .record_mut(thing_name, index)
                .expect("the index came from this store"),
        );
    }

    /// Put one COMMAND execution in the SHARED store, exactly as the data plane's
    /// `StartCommandExecution` does: a `QUEUED` record keyed by the `executionId` it
    /// minted, carrying the `commandArn` that makes it a command execution.
    fn seed_command_execution(&self, thing_name: &str, execution_id: &str, command_id: &str) {
        let mut record = JobExecutionRecord::queued(
            execution_id.to_string(),
            thing_name.to_string(),
            None,
            now_epoch_seconds(),
        );
        record.command_arn = Some(format!(
            "arn:aws:iot:{REGION}:{ACCOUNT_ID}:command/{command_id}"
        ));
        let store = self.engine.state::<JobExecutionStore>();
        let mut store = store.lock();
        store
            .try_insert_execution(record)
            .unwrap_or_else(|refusal| panic!("seeding {execution_id}: {refusal}"));
    }

    /// The status the SHARED store holds for (thing, job) — what the device polling the
    /// Jobs data plane would see.
    fn stored_status(&self, thing_name: &str, job_id: &str) -> String {
        let store = self.engine.state::<JobExecutionStore>();
        let store = store.lock();
        store
            .execution_index(thing_name, job_id, None)
            .and_then(|index| store.record(thing_name, index))
            .map(|record| record.status.clone())
            .unwrap_or_default()
    }
}

/// The ARN a `targets` entry names a device by.
fn target_arn(thing_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/{thing_name}")
}

/// One row of the listing table: a job and the devices it targets.
struct JobCase {
    case: &'static str,
    job_id: &'static str,
    things: &'static [&'static str],
}

/// RULE: the listing reports every execution the store holds for the named device, each
/// the stored record projected onto `JobExecutionSummaryForThing`, in the store's enqueue
/// order — and a device the jobs never targeted answers with an empty list.
#[tokio::test]
async fn a_devices_executions_are_listed_from_the_store() {
    let emulator = Emulator::new();

    // The rows differ in how many devices one job targets and in how many jobs one device
    // is targeted by, which is what the per-device expectations below span.
    let jobs: &[JobCase] = &[
        JobCase {
            case: "one device",
            job_id: "listing-one",
            things: &["lister-a"],
        },
        JobCase {
            case: "three devices",
            job_id: "listing-three",
            things: &["lister-a", "lister-b", "lister-c"],
        },
        JobCase {
            case: "the first device again",
            job_id: "listing-again",
            things: &["lister-a"],
        },
    ];

    for thing in ["lister-a", "lister-b", "lister-c", "lister-untargeted"] {
        emulator.create_thing(thing).await;
    }
    let before = now_epoch_seconds();
    for job in jobs {
        let created = emulator.create_job(job.job_id, job.things).await;
        assert_eq!(
            created.body["jobId"].as_str(),
            Some(job.job_id),
            "case {:?}: the create must report the job it made, got {:?}",
            job.case,
            created.body
        );
    }
    let after = now_epoch_seconds();

    // (case, device, the jobs whose executions it must report, in order)
    let expected: &[(&str, &str, &[&str])] = &[
        (
            "a device three jobs targeted",
            "lister-a",
            &["listing-one", "listing-three", "listing-again"],
        ),
        ("a device one job targeted", "lister-b", &["listing-three"]),
        ("another device of that job", "lister-c", &["listing-three"]),
        ("a device no job targeted", "lister-untargeted", &[]),
        ("a device that was never created", "lister-absent", &[]),
    ];

    for (case, thing, job_ids) in expected {
        let listed = emulator.list(thing).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: listing {thing}'s executions must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.job_ids(),
            job_ids
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected {job_ids:?} in enqueue order, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.next_token(),
            None,
            "case {case:?}: a page holding every execution carries no nextToken, got {:?}",
            listed.body
        );

        for index in 0..job_ids.len() {
            let queued_at = listed.summary_member(index, "queuedAt").as_i64();
            assert!(
                queued_at.is_some_and(|seconds| (before..=after).contains(&seconds)),
                "case {case:?}: entry {index}'s queuedAt must be when the job was created \
                 ({before}..={after}), got {:?}",
                listed.summary_member(index, "queuedAt")
            );
            assert_eq!(
                (
                    listed.summary_member(index, "status"),
                    listed.summary_member(index, "executionNumber"),
                    listed.summary_member(index, "lastUpdatedAt"),
                    listed.summary_member(index, "startedAt"),
                    listed.summary_member(index, "retryAttempt"),
                ),
                (
                    json!(STATUS_QUEUED),
                    json!(1),
                    json!(queued_at),
                    Value::Null,
                    Value::Null,
                ),
                "case {case:?}: entry {index} must be the queued execution the create \
                 made, with no start and no retry, got {:?}",
                listed.entries().get(index)
            );
        }
    }

    // ROUND TRIP: the single-execution reader of the same record agrees with the listing.
    for job_id in ["listing-one", "listing-three", "listing-again"] {
        let read = emulator.describe("lister-a", job_id).await;
        assert_eq!(
            (
                read.status,
                read.body["execution"]["jobId"].as_str(),
                read.body["execution"]["status"].as_str(),
            ),
            (200, Some(job_id), Some(STATUS_QUEUED)),
            "case {job_id:?}: DescribeJobExecution must report the listed execution, \
             got {} {:?}",
            read.status,
            read.body
        );
    }
}

/// RULE: `status` and `jobId` narrow the listing to the executions they select; a filter
/// nothing matches is an empty list.
#[tokio::test]
async fn the_status_and_job_filters_select_executions() {
    let emulator = Emulator::new();
    let thing = "filter-device";
    emulator.create_thing(thing).await;

    // One job per `JobExecutionStatus` value, each moved to that status in the SHARED
    // store the way the device-facing plane moves it — so the table covers the whole
    // catalog, not only the statuses a create leaves behind.
    for status in JOB_EXECUTION_STATUSES {
        let job_id = format!("filter-{}", status.to_lowercase().replace('_', "-"));
        emulator.create_job(&job_id, &[thing]).await;
        emulator.with_stored_execution(thing, &job_id, |record| {
            record.status = (*status).to_string();
        });
    }

    for status in JOB_EXECUTION_STATUSES {
        let job_id = format!("filter-{}", status.to_lowercase().replace('_', "-"));
        let listed = emulator
            .list_with_query(thing, &format!("?status={status}"))
            .await;
        assert_eq!(
            (listed.status, listed.job_ids()),
            (200, vec![job_id.clone()]),
            "case {status:?}: the status filter must select exactly that execution, \
             got {} {:?}",
            listed.status,
            listed.body
        );
    }

    // (case, query, the jobs it must report)
    let rows: &[(&str, String, Vec<String>)] = &[
        (
            "no filter at all",
            String::new(),
            JOB_EXECUTION_STATUSES
                .iter()
                .map(|status| format!("filter-{}", status.to_lowercase().replace('_', "-")))
                .collect(),
        ),
        (
            "one job",
            "?jobId=filter-removed".to_string(),
            vec!["filter-removed".to_string()],
        ),
        (
            "one job in its own status",
            "?jobId=filter-removed&status=REMOVED".to_string(),
            vec!["filter-removed".to_string()],
        ),
        (
            "one job in another status",
            "?jobId=filter-removed&status=QUEUED".to_string(),
            Vec::new(),
        ),
        (
            // `namespaceId` names the MQTT topic namespace a job's notifications are
            // published under, which this emulator does not publish and no creator of a
            // job records, so it selects no subset of the device's executions.
            "a namespaceId",
            "?namespaceId=greengrass".to_string(),
            JOB_EXECUTION_STATUSES
                .iter()
                .map(|status| format!("filter-{}", status.to_lowercase().replace('_', "-")))
                .collect(),
        ),
    ];

    for (case, query, expected) in rows {
        let listed = emulator.list_with_query(thing, query).await;
        assert_eq!(
            (listed.status, listed.job_ids()),
            (200, expected.clone()),
            "case {case:?}: expected {expected:?}, got {} {:?}",
            listed.status,
            listed.body
        );
    }
}

/// RULE: `maxResults` and `nextToken` walk ONE order — every execution of the device
/// exactly once, whatever the page size — and a `nextToken` that is not a token of this
/// listing is refused `InvalidRequestException` (400).
#[tokio::test]
async fn pages_walk_every_execution_exactly_once() {
    let emulator = Emulator::new();
    let thing = "paging-device";
    emulator.create_thing(thing).await;

    let job_ids: Vec<String> = (0..7).map(|index| format!("paging-{index}")).collect();
    for job_id in &job_ids {
        emulator.create_job(job_id, &[thing]).await;
    }

    // Page sizes spanning one execution per page, a size that does not divide the total,
    // the exact total, and each published bound of `LaserMaxResults` (`range 1..250`).
    for page_size in [1, 2, 3, 7, MAX_RESULTS_CEILING] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match token.as_deref() {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let listed = emulator.list_with_query(thing, &query).await;
            assert_eq!(
                listed.status, 200,
                "case maxResults={page_size}: page {pages} must answer 200, got {} {:?}",
                listed.status, listed.body
            );
            assert!(
                listed.entries().len() <= page_size as usize,
                "case maxResults={page_size}: page {pages} must hold at most {page_size} \
                 entries, got {}",
                listed.entries().len()
            );
            walked.extend(listed.job_ids());
            pages += 1;
            assert!(
                pages <= job_ids.len() + 1,
                "case maxResults={page_size}: the walk must terminate, got {pages} pages \
                 for {} executions",
                job_ids.len()
            );
            match listed.next_token() {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, job_ids,
            "case maxResults={page_size}: the pages must cover every execution exactly \
             once in one order, got {walked:?}"
        );
    }

    // A token this listing issued, replayed against a DIFFERENT filter set: it names a
    // position of another listing, so it is refused rather than resuming that one.
    let first_page = emulator.list_with_query(thing, "?maxResults=2").await;
    let token = first_page
        .next_token()
        .expect("a truncated listing carries a nextToken");

    // (case, query)
    let refused: &[(&str, String)] = &[
        (
            "a nextToken that is not a token of any listing",
            "?nextToken=not-a-token".to_string(),
        ),
        (
            "a nextToken that is hex but names no position",
            format!("?nextToken={}", ::hex::encode("nonsense")),
        ),
        (
            "this listing's token under another filter",
            format!("?maxResults=2&status=QUEUED&nextToken={token}"),
        ),
        (
            "this listing's token under a job filter",
            format!("?maxResults=2&jobId=paging-0&nextToken={token}"),
        ),
    ];

    for (case, query) in refused {
        let listed = emulator.list_with_query(thing, query).await;
        assert_eq!(
            (listed.status, listed.error_code.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: expected 400 InvalidRequestException, got {} {:?} with {:?}",
            listed.status,
            listed.error_code,
            listed.body
        );
    }

    // Each `LaserMaxResults` bound, and one past each: the range is modelled, so a page
    // size outside it is refused before this listing runs.
    for (case, page_size, expected) in [
        ("the lower bound", 1, 200),
        ("one below the lower bound", 0, 400),
        ("the upper bound", MAX_RESULTS_CEILING, 200),
        ("one past the upper bound", MAX_RESULTS_CEILING + 1, 400),
    ] {
        let listed = emulator
            .list_with_query(thing, &format!("?maxResults={page_size}"))
            .await;
        assert_eq!(
            listed.status, expected,
            "case {case:?}: maxResults={page_size} must answer {expected}, got {} {:?}",
            listed.status, listed.body
        );
    }
}

/// RULE: a `jobId` naming no job of the account has no execution anywhere, so the listing
/// is refused 404 `ResourceNotFoundException` with the jobs plane's own sentence.
#[tokio::test]
async fn a_job_filter_naming_no_job_is_refused() {
    let emulator = Emulator::new();
    let thing = "missing-job-device";
    emulator.create_thing(thing).await;
    emulator.create_job("present-job", &[thing]).await;

    // The rows differ in whether the device has executions at all and in how the unknown
    // id relates to the one that does exist.
    for (case, thing_name, job_id) in [
        ("a device with executions", thing, "absent-job"),
        ("a device with none", "no-execution-device", "absent-job"),
        ("the known id with a suffix", thing, "present-job-2"),
    ] {
        let listed = emulator
            .list_with_query(thing_name, &format!("?jobId={job_id}"))
            .await;
        assert_eq!(
            (listed.status, listed.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {:?} with {:?}",
            listed.status,
            listed.error_code,
            listed.body
        );
        assert_eq!(
            listed.message(),
            format!("Job execution cannot be found as the Job {job_id} does not exist."),
            "case {case:?}: expected the jobs plane's missing-job sentence, got {:?}",
            listed.body
        );
    }

    // The job that DOES exist is listed on the device it targeted and nowhere else.
    for (case, thing_name, expected) in [
        ("the targeted device", thing, vec!["present-job".to_string()]),
        ("another device", "no-execution-device", Vec::new()),
    ] {
        let listed = emulator
            .list_with_query(thing_name, "?jobId=present-job")
            .await;
        assert_eq!(
            (listed.status, listed.job_ids()),
            (200, expected.clone()),
            "case {case:?}: expected {expected:?}, got {} {:?}",
            listed.status,
            listed.body
        );
    }
}

/// RULE: the listing reports the status the CLOCK decided — an execution whose armed timer
/// has passed is `TIMED_OUT`, stamped at the deadline, while a twin whose timer is still
/// ahead is untouched — and a COMMAND execution the same store holds is not a job
/// execution of this plane.
#[tokio::test]
async fn a_due_timeout_is_reported_and_a_command_execution_is_not_listed() {
    let emulator = Emulator::new();
    let thing = "timer-device";
    emulator.create_thing(thing).await;

    let now = now_epoch_seconds();
    // (case, job id, the deadline its timer is armed at, the status it must be listed in)
    let rows: &[(&str, &str, i64, &str)] = &[
        ("a deadline the clock passed", "timer-past", now - 60, STATUS_TIMED_OUT),
        ("a deadline reached exactly now", "timer-now", now, STATUS_TIMED_OUT),
        ("a deadline still ahead", "timer-future", now + 3_600, STATUS_QUEUED),
    ];

    for (_, job_id, deadline, _) in rows {
        emulator.create_job(job_id, &[thing]).await;
        emulator.with_stored_execution(thing, job_id, |record| {
            record.execution_timeout_expires_at = Some(*deadline);
        });
    }
    // A finished execution has no deadline left to reach, so its armed timer moves it
    // nowhere.
    emulator.create_job("timer-rejected", &[thing]).await;
    emulator.with_stored_execution(thing, "timer-rejected", |record| {
        record.status = STATUS_REJECTED.to_string();
        record.execution_timeout_expires_at = Some(now - 60);
    });
    // A command execution of the same device, in the same store, keyed by the id a
    // command start minted.
    emulator.seed_command_execution(thing, "command-execution-1", "reboot-command");

    let listed = emulator.list(thing).await;
    assert_eq!(
        listed.status, 200,
        "listing the device's executions must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.job_ids(),
        vec![
            "timer-past".to_string(),
            "timer-now".to_string(),
            "timer-future".to_string(),
            "timer-rejected".to_string(),
        ],
        "the command execution must not be listed as a job execution, got {:?}",
        listed.body
    );

    for (index, (case, job_id, deadline, expected_status)) in rows.iter().enumerate() {
        assert_eq!(
            listed.summary_member(index, "status"),
            json!(expected_status),
            "case {case:?}: expected {job_id} listed {expected_status}, got {:?}",
            listed.entries().get(index)
        );
        assert_eq!(
            emulator.stored_status(thing, job_id),
            (*expected_status).to_string(),
            "case {case:?}: the device polling the shared store must see the same status"
        );
        if *expected_status == STATUS_TIMED_OUT {
            assert_eq!(
                listed.summary_member(index, "lastUpdatedAt"),
                json!(*deadline),
                "case {case:?}: a timed-out execution is stamped at its deadline \
                 {deadline}, got {:?}",
                listed.entries().get(index)
            );
        }
    }
    assert_eq!(
        (
            listed.summary_member(3, "status"),
            emulator.stored_status(thing, "timer-rejected"),
        ),
        (json!(STATUS_REJECTED), STATUS_REJECTED.to_string()),
        "case \"a finished execution with an armed timer\": a terminal status is never \
         moved, got {:?}",
        listed.entries().get(3)
    );

    // Reading does not change what a later caller sees: the second listing is the first.
    let again = emulator.list(thing).await;
    assert_eq!(
        again.body, listed.body,
        "a second listing must report exactly what the first did, got {:?}",
        again.body
    );
    assert_eq!(
        (
            again.summary_member(0, "status"),
            again.summary_member(2, "status"),
        ),
        (json!(STATUS_TIMED_OUT), json!(STATUS_QUEUED)),
        "case \"a re-read\": the clock's transition is recorded once and the twin stays \
         queued, got {:?}",
        again.body
    );
    // The command execution is still in the shared store; it is simply not a job
    // execution of this plane.
    assert_eq!(
        emulator.stored_status(thing, "command-execution-1"),
        STATUS_QUEUED.to_string(),
        "the listing must leave the command execution the data plane serves untouched"
    );
    // One device's in-progress execution is listed as such, which is the status the
    // device reports once it starts the work.
    emulator.with_stored_execution(thing, "timer-future", |record| {
        record.start(now);
    });
    let started = emulator
        .list_with_query(thing, &format!("?status={STATUS_IN_PROGRESS}"))
        .await;
    assert_eq!(
        (started.status, started.job_ids()),
        (200, vec!["timer-future".to_string()]),
        "case \"a started execution\": expected it listed IN_PROGRESS, got {} {:?}",
        started.status,
        started.body
    );
    assert_eq!(
        started.summary_member(0, "startedAt"),
        json!(now),
        "case \"a started execution\": startedAt must be when the device started it, \
         got {:?}",
        started.entries().first()
    );
}
