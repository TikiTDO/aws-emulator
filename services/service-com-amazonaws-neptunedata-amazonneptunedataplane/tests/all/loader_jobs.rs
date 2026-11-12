// SPDX-License-Identifier: CC0-1.0

//! The neptunedata bulk-loader job list, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: `ListLoaderJobs` reports the load ids of the loader jobs the
//! registry in `crate::state` holds as ACTIVE — the jobs the loader still owes an
//! outcome — with the queued load requests among them unless the request excludes them,
//! newest admission first and at most `limit` of them. A job that has finished is not
//! active, so its id is never listed, and nothing the registry does not hold is.
//!
//! The loader EXECUTION endpoint (`StartLoaderJob`) is deliberately still 501, which
//! [`start_loader_job_is_refused_and_records_no_job`] pins: a bulk load reads data files
//! and writes them into a graph, and this emulator has no graph store to write them into,
//! so a load id for a load nothing performed would be a fabrication.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_neptunedata_amazonneptunedataplane::state::{
    ComAmazonawsNeptunedataAmazonneptunedataplaneState, LoaderJobRecord, LoaderJobState,
};
use service_com_amazonaws_neptunedata_amazonneptunedataplane::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service. `neptune-db` is neptunedata's sigv4
/// signing name, and it is what routes a request to this service.
const SIGNED_FOR_NEPTUNE_DB: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/neptune-db/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `ListLoaderJobsOutput$status` for a job-list request the loader served, spelled out
/// here rather than imported so a wrong value in `src/state.rs` fails this test.
const JOB_LIST_SERVED: &str = "200 OK";

/// The published bounds of `ListLoaderJobsInput$limit`.
/// Source: smithy_query(query="shape", shape="ListLoaderJobsInput") — `limit` targets
/// PositiveInteger with range {min: 1, max: 100}, and its documentation names 100 as the
/// default as well as the maximum.
const LIMIT_BOUNDS: (i32, i32) = (1, 100);

/// One emulator: a router carrying only this service, over one shared engine. The engine
/// is handed back so a test can seed the registry the way a loader that really loads
/// would.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "neptune-db.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_NEPTUNE_DB);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn list_jobs(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/loader{query}"), None).await
}

/// The load ids a list answer carries, in the order it carries them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["payload"]["loadIds"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect()
}

/// One seeded loader job: the record to put in the registry, and how far it has got.
#[derive(Debug, Clone, Copy)]
struct Job {
    load_id: &'static str,
    state: LoaderJobState,
    /// Seconds before "now" the loader admitted the job.
    admitted_secs_ago: u64,
}

/// The seeded rows. Load ids are the UUID-shaped strings Neptune reports a load under.
/// The rows differ along everything the rule generalises over: how far each job has got
/// (every state the record can hold), and when it was admitted — including two jobs
/// admitted in the SAME instant, so the order the list reports is total rather than
/// whatever the registry happens to iterate.
const JOBS: [Job; 7] = [
    Job {
        load_id: "3a8b1f00-0000-4000-8000-000000000001",
        state: LoaderJobState::Loading,
        admitted_secs_ago: 30,
    },
    Job {
        load_id: "3a8b1f00-0000-4000-8000-000000000002",
        state: LoaderJobState::Queued,
        admitted_secs_ago: 20,
    },
    // Admitted in the same instant as the row above, and listed after it because the
    // load id breaks the tie.
    Job {
        load_id: "3a8b1f00-0000-4000-8000-000000000007",
        state: LoaderJobState::Loading,
        admitted_secs_ago: 20,
    },
    Job {
        load_id: "3a8b1f00-0000-4000-8000-000000000003",
        state: LoaderJobState::Queued,
        admitted_secs_ago: 10,
    },
    Job {
        load_id: "3a8b1f00-0000-4000-8000-000000000004",
        state: LoaderJobState::Completed,
        admitted_secs_ago: 40,
    },
    Job {
        load_id: "3a8b1f00-0000-4000-8000-000000000005",
        state: LoaderJobState::Failed,
        admitted_secs_ago: 50,
    },
    Job {
        load_id: "3a8b1f00-0000-4000-8000-000000000006",
        state: LoaderJobState::Cancelled,
        admitted_secs_ago: 60,
    },
];

/// Put [`JOBS`] in the registry, the way a loader that really loads would.
fn seed(engine: &engine::InMemoryEngine) {
    let now = SystemTime::now();
    let state = engine.state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>();
    let mut store = state.lock();
    for job in &JOBS {
        let record = LoaderJobRecord::admitted(
            job.load_id,
            now - Duration::from_secs(job.admitted_secs_ago),
            job.state,
        );
        store.loader_jobs.insert(record.load_id.clone(), record);
    }
}

/// The ids of the seeded rows that are active (queued or loading), newest first and ties
/// broken by load id — what the list must report when the request asks for everything.
fn active_ids_newest_first(include_queued: bool) -> Vec<&'static str> {
    let mut active: Vec<&Job> = JOBS
        .iter()
        .filter(|job| {
            let queued = matches!(job.state, LoaderJobState::Queued);
            matches!(job.state, LoaderJobState::Queued | LoaderJobState::Loading)
                && (include_queued || !queued)
        })
        .collect();
    active.sort_by_key(|job| (job.admitted_secs_ago, job.load_id));
    active.into_iter().map(|job| job.load_id).collect()
}

#[tokio::test]
async fn an_empty_registry_lists_no_load_ids() {
    let Emulator { router, .. } = emulator();

    let answer = list_jobs(&router, "").await;
    assert_eq!(
        answer.status, 200,
        "GET /loader: expected 200, got {} with body {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["status"].as_str(),
        Some(JOB_LIST_SERVED),
        "status: expected the served job-list status, got {:?}",
        answer.body["status"]
    );
    assert_eq!(
        listed_ids(&answer),
        Vec::<String>::new(),
        "an engine that has admitted no load must list no load ids, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn the_list_reports_the_active_jobs_the_registry_holds() {
    let Emulator { router, engine } = emulator();
    seed(&engine);

    // The rows: how the request spells `includeQueuedLoads`, and the ids that must come
    // back. An absent member defaults to TRUE, so the queued requests are listed; FALSE
    // excludes exactly them. A finished job is never listed whatever the request asks.
    let cases: [(&str, Vec<&'static str>); 4] = [
        ("", active_ids_newest_first(true)),
        ("?includeQueuedLoads=true", active_ids_newest_first(true)),
        ("?includeQueuedLoads=false", active_ids_newest_first(false)),
        (
            "?includeQueuedLoads=true&limit=100",
            active_ids_newest_first(true),
        ),
    ];

    for (query, expected) in cases {
        let answer = list_jobs(&router, query).await;
        assert_eq!(
            answer.status, 200,
            "case {query:?}: expected 200, got {} with body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            listed_ids(&answer),
            expected,
            "case {query:?}: expected {expected:?}, got {:?}",
            answer.body
        );
    }

    // No request lists a job that has finished.
    let listed = listed_ids(&list_jobs(&router, "?includeQueuedLoads=true").await);
    for job in JOBS.iter().filter(|job| {
        matches!(
            job.state,
            LoaderJobState::Completed | LoaderJobState::Failed | LoaderJobState::Cancelled
        )
    }) {
        assert!(
            !listed.contains(&job.load_id.to_string()),
            "case {:?}: a {:?} job is not active, so its id must not be listed, got {listed:?}",
            job.load_id,
            job.state
        );
    }
}

#[tokio::test]
async fn limit_takes_the_newest_jobs_and_is_bounded_by_the_published_range() {
    let Emulator { router, engine } = emulator();
    seed(&engine);

    let newest_first = active_ids_newest_first(true);
    let (min_limit, max_limit) = LIMIT_BOUNDS;

    // Inside the published range the answer is the first `limit` ids of the same order,
    // so the newest jobs are the ones a bounded request sees. Both published bounds are
    // among the rows, and `limit` 2 and 3 straddle the two jobs admitted in the same
    // instant, so the tie-break decides which of them a bounded request sees.
    for limit in [min_limit, 2, 3, max_limit] {
        let answer = list_jobs(&router, &format!("?limit={limit}")).await;
        assert_eq!(
            answer.status, 200,
            "case limit={limit}: expected 200, got {} with body {:?}",
            answer.status, answer.body
        );
        let wanted: Vec<&str> = newest_first
            .iter()
            .take(usize::try_from(limit).unwrap_or(0))
            .copied()
            .collect();
        assert_eq!(
            listed_ids(&answer),
            wanted,
            "case limit={limit}: expected {wanted:?}, got {:?}",
            answer.body
        );
    }

    // One past each published bound is outside the modelled range, so the request is
    // refused rather than served a guess, and the refusal names the member and the bound
    // the value missed.
    for (limit, named_bound) in [(min_limit - 1, min_limit), (max_limit + 1, max_limit)] {
        let answer = list_jobs(&router, &format!("?limit={limit}")).await;
        assert_eq!(
            answer.status, 400,
            "case limit={limit}: a limit outside {min_limit}..={max_limit} must be refused, got {} with body {:?}",
            answer.status, answer.body
        );
        let message = answer.body["message"]
            .as_str()
            .or_else(|| answer.body["detailedMessage"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains("limit") && message.contains(&named_bound.to_string()),
            "case limit={limit}: the refusal must name the member and the bound {named_bound}, got {:?}",
            answer.body
        );
    }

    // The refusal left the registry exactly as it was.
    let listed = list_jobs(&router, "?includeQueuedLoads=true").await;
    assert_eq!(
        listed_ids(&listed),
        newest_first,
        "a refused limit must not change the jobs the list reports, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn listing_the_jobs_changes_nothing_a_later_caller_sees() {
    let Emulator { router, engine } = emulator();
    seed(&engine);

    let first = list_jobs(&router, "?includeQueuedLoads=true").await;
    // A narrower read in between must not consume, reorder or retire anything.
    let _ = list_jobs(&router, "?includeQueuedLoads=false&limit=1").await;
    let again = list_jobs(&router, "?includeQueuedLoads=true").await;

    assert_eq!(
        listed_ids(&again),
        listed_ids(&first),
        "a read must not change what a later caller sees: {:?} then {:?}",
        first.body,
        again.body
    );

    let state = engine.state::<ComAmazonawsNeptunedataAmazonneptunedataplaneState>();
    assert_eq!(
        state.lock().loader_jobs.len(),
        JOBS.len(),
        "every seeded job must still be in the registry after the reads"
    );
}

/// The loader EXECUTION endpoint is still a stub, and must stay one until this emulator
/// can really load data: a load id for a load nothing performed would be fabricated. A
/// 501 says "nobody has written it yet", which is the truth, and it must leave nothing
/// behind in the registry.
#[tokio::test]
async fn start_loader_job_is_refused_and_records_no_job() {
    let Emulator { router, .. } = emulator();

    let answer = call(
        &router,
        "POST",
        "/loader",
        Some(serde_json::json!({
            "source": "s3://example-bucket/vertices/",
            "format": "csv",
            "region": "us-east-1",
            "iamRoleArn": "arn:aws:iam::000000000001:role/NeptuneLoadFromS3",
        })),
    )
    .await;
    assert_eq!(
        answer.status, 501,
        "POST /loader: expected the scaffold's 501 while nothing can perform a load, got {} with body {:?}",
        answer.status, answer.body
    );

    let listed = list_jobs(&router, "?includeQueuedLoads=true").await;
    assert_eq!(
        listed_ids(&listed),
        Vec::<String>::new(),
        "a refused load must not record a job, got {:?}",
        listed.body
    );
}
