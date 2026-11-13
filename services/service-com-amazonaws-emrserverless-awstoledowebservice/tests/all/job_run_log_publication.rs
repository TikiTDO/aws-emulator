// SPDX-License-Identifier: CC0-1.0

//! LOG PUBLICATION: what an application's `monitoringConfiguration` makes a finished job
//! run write into Amazon S3, end to end over HTTP across both services.
//!
//! RULE: a job run submitted under a monitoring configuration whose
//! `s3MonitoringConfiguration.logUri` names an Amazon S3 destination publishes its driver
//! log into that destination once the run reaches a state it never leaves, so a client
//! that reads the destination afterwards finds the run's log there — whatever terminal
//! state the run reached — and a run submitted under no such configuration publishes
//! nothing.
//!
//! The artifact is keyed by the application and the job run under the configured URI and
//! is a gzip stream whose text is what this service recorded about the run. The key layout
//! (`<logUri>applications/<applicationId>/jobs/<jobRunId>/<WORKER_TYPE>/stderr.gz`) is
//! RECORDED behaviour — the Smithy model states no key scheme — and is written out by hand
//! here, so a changed layout in `src/` fails this suite.
//!
//! The rows differ along what the rule generalises over: WHO configures the destination
//! (the application, the run's own `configurationOverrides`, both at once, neither), the
//! FORM of the configured URI (a prefix with and without a trailing separator, a nested
//! prefix, a bucket with no prefix at all), the ENGINE whose driver the artifact belongs to
//! (Spark and Hive, which name different worker types), and the TERMINAL STATE the run
//! reached (the one the clock takes it to, and the one a cancel takes it to).
//!
//! Amazon S3 is mounted on the SAME engine this service runs on, exactly as the assembled
//! emulator mounts it, and every artifact is read back through the object store's own
//! `ListObjectsV2` and `GetObject`: what is asserted is what that service persisted and
//! not a copy of it.

use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::EmulatedService;

use super::application_lifecycle::{
    call, cancel_job_run, create_application, get_job_run, start_job_run, Answer, EXECUTION_ROLE,
};

/// The Region every request of this suite is signed for — the one the shared helpers in
/// [`super::application_lifecycle`] sign for, so the object store this service publishes
/// through is addressed in the same Region the test reads it in.
const REGION: &str = "eu-west-1";

/// The bucket every row publishes into.
const BUCKET: &str = "emr-serverless-job-logs";

/// The states a `JobRunState` names that a job run never leaves. Source:
/// `smithy_query(query="shape",shape="JobRunState")` — `SUCCESS`, `FAILED` and `CANCELLED`
/// are the three a run cannot move out of. Written out here rather than imported, so a
/// wrong set in `src/` cannot make this suite pass.
const TERMINAL_STATES: &[&str] = &["SUCCESS", "FAILED", "CANCELLED"];

/// The worker type a Spark application's driver runs as, which is the key segment its
/// artifacts are published under (recorded behaviour, from the issue's recorded layout).
const SPARK_DRIVER: &str = "SPARK_DRIVER";

/// The same, for a Hive application's driver.
const HIVE_DRIVER: &str = "HIVE_DRIVER";

/// The artifact a finished run publishes.
const STDERR_OBJECT: &str = "stderr.gz";

/// How long a row waits for a condition it polls for: the whole compressed run window,
/// the transitions around it and generous slack for a loaded machine.
const BUDGET: Duration = Duration::from_secs(60);

/// How often it looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The first two bytes of a gzip stream (RFC 1952's identification fields).
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// One emulator carrying this service and Amazon S3 over ONE shared engine, exactly as the
/// production router builds them.
fn emulator_with_object_store() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(EmulatedService::new(engine));
    router
}

/// One path-style Amazon S3 call, answered as the whole HTTP response: S3 is a REST
/// service, so the bucket and the key live in the URI and the body is the object's own
/// BYTES — which is what a gzip artifact has to be read as.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str) -> (u16, Bytes) {
    s3_with_body(router, method, path, Bytes::new()).await
}

/// The same, for a call that carries a request body (a bucket create's
/// `CreateBucketConfiguration`).
async fn s3_with_body(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Bytes,
) -> (u16, Bytes) {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    if !body.is_empty() {
        builder = builder.header("content-type", "application/xml");
    }
    let request = builder
        .body(body)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    (response.status().as_u16(), response.into_body())
}

/// Create [`BUCKET`], so the destination every row names resolves.
///
/// The create names the Region it is sent to in its `CreateBucketConfiguration`: a
/// regional endpoint refuses a create that names no location constraint.
async fn given_bucket(router: &MultiServiceRouter) {
    let configuration = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let (status, body) = s3_with_body(
        router,
        "PUT",
        &format!("/{BUCKET}"),
        Bytes::from(configuration),
    )
    .await;
    assert!(
        status == 200 || status == 409,
        "creating s3://{BUCKET} must succeed or report it is already there, got {status} \
         with {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// The keys of every object under `prefix` in [`BUCKET`], from `ListObjectsV2`.
async fn keys_under(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let query = format!("/{BUCKET}?list-type=2&prefix={prefix}");
    let (status, listing) = s3(router, "GET", &query).await;
    let listing = String::from_utf8_lossy(&listing).to_string();
    assert_eq!(
        status, 200,
        "listing s3://{BUCKET}/{prefix} must succeed, got {status} with {listing}"
    );
    listing
        .split("<Key>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</Key>").map(|(key, _)| key.to_string()))
        .collect()
}

/// A `monitoringConfiguration` naming `log_uri` as its Amazon S3 log destination.
fn monitoring(log_uri: &str) -> Value {
    json!({ "s3MonitoringConfiguration": { "logUri": log_uri } })
}

/// Poll `GetJobRun` until the run reports a state it never leaves, and answer that read.
///
/// The wait is on the CONDITION, not on a span: it looks again every
/// [`POLL_INTERVAL`] until the run is terminal or the generous [`BUDGET`] passes.
async fn poll_until_terminal(
    router: &MultiServiceRouter,
    case: &str,
    application_id: &str,
    job_run_id: &str,
) -> Answer {
    let deadline = Instant::now() + BUDGET;
    loop {
        let answer = get_job_run(router, application_id, job_run_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: GetJobRun expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let state = answer
            .at("/jobRun/state")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if TERMINAL_STATES.contains(&state.as_str()) {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job run must reach a terminal state, last saw {state:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Poll `ListObjectsV2` until `prefix` holds at least one object, and answer the keys it
/// holds then (or the empty listing when the budget passes).
async fn poll_until_published(
    router: &MultiServiceRouter,
    prefix: &str,
) -> Vec<String> {
    let deadline = Instant::now() + BUDGET;
    loop {
        let keys = keys_under(router, prefix).await;
        if !keys.is_empty() || Instant::now() >= deadline {
            return keys;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The text of a published gzip artifact, with the framing asserted first.
async fn published_text(router: &MultiServiceRouter, case: &str, key: &str) -> String {
    let (status, body) = s3(router, "GET", &format!("/{BUCKET}/{key}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetObject on {key} expected 200, got {status}"
    );
    assert_eq!(
        body.get(..2),
        Some(GZIP_MAGIC.as_slice()),
        "case {case:?}: {key} must be a gzip stream, got the first bytes {:?}",
        body.get(..2)
    );
    let mut text = String::new();
    ::flate2::read::GzDecoder::new(body.as_ref())
        .read_to_string(&mut text)
        .unwrap_or_else(|error| panic!("case {case:?}: {key} is not a gzip stream: {error}"));
    text
}

/// One row: what the application and the run configure, and what that has to publish.
struct Row {
    case: &'static str,
    /// `CreateApplicationRequest$type`.
    engine_type: &'static str,
    /// The application's `monitoringConfiguration`, or `None` for one that names none.
    application_monitoring: Option<Value>,
    /// The run's `configurationOverrides`, or `None` for a run that overrides nothing.
    overrides: Option<Value>,
    /// The `jobDriver` the run is submitted with.
    driver: Value,
    /// The key prefix the artifact must be published under, as the configured URI spells
    /// it — `Some("")` is a destination with no prefix at all, `None` a run that must
    /// publish nothing.
    key_prefix: Option<&'static str>,
    /// The worker-type segment the artifact is keyed by.
    worker_type: &'static str,
    /// Whether the row cancels the run instead of letting the clock finish it.
    cancels: bool,
}

/// The `sparkSubmit` driver every Spark row submits. Its `entryPoint` is `@sensitive`, and
/// [`the_published_log_withholds_the_sensitive_driver_members`] is what pins that it stays
/// out of the published text.
fn spark_driver() -> Value {
    json!({ "sparkSubmit": { "entryPoint": SENSITIVE_ENTRY_POINT } })
}

/// The entry point the Spark rows submit, which no published artifact may carry.
const SENSITIVE_ENTRY_POINT: &str = "s3://scripts/secret-ddl.py";

/// The `hive` driver every Hive row submits; its `query` is `@sensitive` too.
fn hive_driver() -> Value {
    json!({ "hive": { "query": SENSITIVE_QUERY } })
}

/// See [`SENSITIVE_ENTRY_POINT`].
const SENSITIVE_QUERY: &str = "SELECT secret FROM vault";

/// The rows the rule is asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the application configures a prefix with a trailing separator",
            engine_type: "SPARK",
            application_monitoring: Some(monitoring(&format!("s3://{BUCKET}/logs/"))),
            overrides: None,
            driver: spark_driver(),
            key_prefix: Some("logs/"),
            worker_type: SPARK_DRIVER,
            cancels: false,
        },
        Row {
            case: "the same prefix written without a trailing separator",
            engine_type: "SPARK",
            application_monitoring: Some(monitoring(&format!("s3://{BUCKET}/plain"))),
            overrides: None,
            driver: spark_driver(),
            key_prefix: Some("plain/"),
            worker_type: SPARK_DRIVER,
            cancels: false,
        },
        Row {
            case: "a nested prefix",
            engine_type: "SPARK",
            application_monitoring: Some(monitoring(&format!("s3://{BUCKET}/a/b/c/"))),
            overrides: None,
            driver: spark_driver(),
            key_prefix: Some("a/b/c/"),
            worker_type: SPARK_DRIVER,
            cancels: false,
        },
        Row {
            case: "a bucket with no prefix at all",
            engine_type: "SPARK",
            application_monitoring: Some(monitoring(&format!("s3://{BUCKET}"))),
            overrides: None,
            driver: spark_driver(),
            key_prefix: Some(""),
            worker_type: SPARK_DRIVER,
            cancels: false,
        },
        Row {
            case: "a Hive application, whose driver is a different worker type",
            engine_type: "HIVE",
            application_monitoring: Some(monitoring(&format!("s3://{BUCKET}/hive/"))),
            overrides: None,
            driver: hive_driver(),
            key_prefix: Some("hive/"),
            worker_type: HIVE_DRIVER,
            cancels: false,
        },
        Row {
            case: "the run's own overrides, on an application configuring none",
            engine_type: "SPARK",
            application_monitoring: None,
            overrides: Some(json!({
                "monitoringConfiguration": monitoring(&format!("s3://{BUCKET}/by-run/"))
            })),
            driver: spark_driver(),
            key_prefix: Some("by-run/"),
            worker_type: SPARK_DRIVER,
            cancels: false,
        },
        Row {
            case: "the run's overrides winning over the application's destination",
            engine_type: "SPARK",
            application_monitoring: Some(monitoring(&format!("s3://{BUCKET}/by-app/"))),
            overrides: Some(json!({
                "monitoringConfiguration": monitoring(&format!("s3://{BUCKET}/overridden/"))
            })),
            driver: spark_driver(),
            key_prefix: Some("overridden/"),
            worker_type: SPARK_DRIVER,
            cancels: false,
        },
        Row {
            case: "a run the caller cancels, which still publishes its log",
            engine_type: "SPARK",
            application_monitoring: Some(monitoring(&format!("s3://{BUCKET}/cancelled/"))),
            overrides: None,
            driver: spark_driver(),
            key_prefix: Some("cancelled/"),
            worker_type: SPARK_DRIVER,
            cancels: true,
        },
        Row {
            case: "no monitoring configuration anywhere, which publishes nothing",
            engine_type: "SPARK",
            application_monitoring: None,
            overrides: None,
            driver: spark_driver(),
            key_prefix: None,
            worker_type: SPARK_DRIVER,
            cancels: false,
        },
    ]
}

/// A `clientToken`-safe rendering of a case label: `ClientToken` is modelled
/// `^[A-Za-z0-9._-]+$`, so a label's spaces and punctuation become `-`.
fn slug(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// What one row's run ended up as: the ids, the terminal read and the keys published.
struct Published {
    application_id: String,
    job_run_id: String,
    terminal: Answer,
    keys: Vec<String>,
}

/// Run one row: create the application, submit the run, take it to a terminal state and
/// answer what reached the object store.
async fn run_row(router: &MultiServiceRouter, row: &Row, index: usize) -> Published {
    // `ClientToken` is bounded at 64 characters, so the row is named by its INDEX rather
    // than by its label; the label is what every assertion reports.
    let token = index;
    let mut create = json!({
        "clientToken": format!("app-{token}"),
        "releaseLabel": "emr-7.1.0",
        "type": row.engine_type,
    });
    if let Some(monitoring) = row.application_monitoring.clone() {
        create["monitoringConfiguration"] = monitoring;
    }
    let created = create_application(router, create).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: CreateApplication expected 200, got {} with {:?}",
        row.case, created.status, created.body
    );
    let application_id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    let mut submit = json!({
        "clientToken": format!("run-{token}"),
        "executionRoleArn": EXECUTION_ROLE,
        "jobDriver": row.driver.clone(),
    });
    if let Some(overrides) = row.overrides.clone() {
        submit["configurationOverrides"] = overrides;
    }
    let submitted = start_job_run(router, &application_id, submit).await;
    assert_eq!(
        submitted.status, 200,
        "case {:?}: StartJobRun expected 200, got {} with {:?}",
        row.case, submitted.status, submitted.body
    );
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    if row.cancels {
        let cancelled = cancel_job_run(router, &application_id, &job_run_id, "").await;
        assert_eq!(
            cancelled.status, 200,
            "case {:?}: CancelJobRun expected 200, got {} with {:?}",
            row.case, cancelled.status, cancelled.body
        );
    }

    let terminal = poll_until_terminal(router, row.case, &application_id, &job_run_id).await;
    let run_prefix = format!(
        "{}applications/{application_id}/jobs/{job_run_id}/",
        row.key_prefix.unwrap_or_default()
    );
    // The publication is performed BY the call that materialised the terminal state, and
    // awaited before that call answers — so the read above is already past it and this
    // listing is taken at a settled instant rather than raced against one. The poll is
    // kept for the rows that must publish, so a slow host cannot fail them.
    let keys = match row.key_prefix {
        None => keys_under(router, &run_prefix).await,
        Some(_) => poll_until_published(router, &run_prefix).await,
    };
    Published {
        application_id,
        job_run_id,
        terminal,
        keys,
    }
}

/// RULE: a configured `s3MonitoringConfiguration.logUri` makes the finished run's driver
/// log EXIST under it, keyed by the application, the run and the driver's worker type, as
/// one gzip stream; a run configured with no such destination publishes nothing.
#[tokio::test]
async fn a_finished_job_run_publishes_its_driver_log_to_the_configured_log_uri() {
    let router = emulator_with_object_store();
    given_bucket(&router).await;

    for (index, row) in rows().into_iter().enumerate() {
        let published = run_row(&router, &row, index).await;
        let Published {
            application_id,
            job_run_id,
            terminal,
            keys,
        } = published;
        let case = row.case;

        let Some(prefix) = row.key_prefix else {
            assert!(
                keys.is_empty(),
                "case {case:?}: a run with no log destination configured must publish \
                 nothing, got {keys:?}"
            );
            continue;
        };

        let expected = format!(
            "{prefix}applications/{application_id}/jobs/{job_run_id}/{}/{STDERR_OBJECT}",
            row.worker_type
        );
        assert_eq!(
            keys,
            vec![expected.clone()],
            "case {case:?}: expected exactly the driver log under the configured URI, got \
             {keys:?}"
        );

        let text = published_text(&router, case, &expected).await;
        let state = terminal
            .at("/jobRun/state")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let state_details = terminal
            .at("/jobRun/stateDetails")
            .as_str()
            .unwrap_or_default()
            .to_string();
        for needle in [
            application_id.as_str(),
            job_run_id.as_str(),
            EXECUTION_ROLE,
            state.as_str(),
            state_details.as_str(),
        ] {
            assert!(
                text.contains(needle),
                "case {case:?}: the published log must report {needle:?}, got {text:?}"
            );
        }
    }
}

/// RULE: the published log is the run's own record and carries none of the driver members
/// the model marks `@sensitive` (`SparkSubmit$entryPoint`, `SparkSubmit$sparkSubmitParameters`,
/// `Hive$query`), whichever driver the run was submitted with.
#[tokio::test]
async fn the_published_log_withholds_the_sensitive_driver_members() {
    let router = emulator_with_object_store();
    given_bucket(&router).await;

    let rows = [
        ("a sparkSubmit driver", "SPARK", spark_driver(), SENSITIVE_ENTRY_POINT, SPARK_DRIVER),
        ("a hive driver", "HIVE", hive_driver(), SENSITIVE_QUERY, HIVE_DRIVER),
    ];
    for (case, engine_type, driver, withheld, worker_type) in rows {
        // Each engine's artifacts land under its own prefix, so the two rows cannot
        // collide in the one bucket.
        let prefix = format!("{engine_type}/");
        let token = slug(case);
        let created = create_application(
            &router,
            json!({
                "clientToken": format!("app-{token}"),
                "releaseLabel": "emr-7.1.0",
                "type": engine_type,
                "monitoringConfiguration": monitoring(&format!("s3://{BUCKET}/{prefix}")),
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateApplication expected 200, got {} with {:?}",
            created.status, created.body
        );
        let application_id = created
            .at("/applicationId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let submitted = start_job_run(
            &router,
            &application_id,
            json!({
                "clientToken": format!("run-{token}"),
                "executionRoleArn": EXECUTION_ROLE,
                "jobDriver": driver,
            }),
        )
        .await;
        assert_eq!(
            submitted.status, 200,
            "case {case:?}: StartJobRun expected 200, got {} with {:?}",
            submitted.status, submitted.body
        );
        let job_run_id = submitted
            .at("/jobRunId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        poll_until_terminal(&router, case, &application_id, &job_run_id).await;
        let run_prefix = format!("{prefix}applications/{application_id}/jobs/{job_run_id}/");
        let keys = poll_until_published(&router, &run_prefix).await;
        let expected = format!("{run_prefix}{worker_type}/{STDERR_OBJECT}");
        assert_eq!(
            keys,
            vec![expected.clone()],
            "case {case:?}: expected the driver log at {expected:?}, got {keys:?}"
        );
        let text = published_text(&router, case, &expected).await;
        assert!(
            !text.contains(withheld),
            "case {case:?}: the published log must withhold the sensitive driver value \
             {withheld:?}, got {text:?}"
        );
    }
}

/// RULE: the artifacts of one finished run are published ONCE, however many calls settle
/// the store afterwards — the publication belongs to the state change, not to the call
/// that observed it.
#[tokio::test]
async fn one_finished_run_publishes_one_artifact_however_often_it_is_read() {
    let router = emulator_with_object_store();
    given_bucket(&router).await;

    let created = create_application(
        &router,
        json!({
            "clientToken": "app-once",
            "releaseLabel": "emr-7.1.0",
            "type": "SPARK",
            "monitoringConfiguration": monitoring(&format!("s3://{BUCKET}/once/")),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case once: CreateApplication expected 200, got {} with {:?}",
        created.status, created.body
    );
    let application_id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let submitted = start_job_run(
        &router,
        &application_id,
        json!({
            "clientToken": "run-once",
            "executionRoleArn": EXECUTION_ROLE,
            "jobDriver": spark_driver(),
        }),
    )
    .await;
    assert_eq!(
        submitted.status, 200,
        "case once: StartJobRun expected 200, got {} with {:?}",
        submitted.status, submitted.body
    );
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    poll_until_terminal(&router, "once", &application_id, &job_run_id).await;

    let run_prefix = format!("once/applications/{application_id}/jobs/{job_run_id}/");
    let first = poll_until_published(&router, &run_prefix).await;
    assert_eq!(
        first.len(),
        1,
        "case once: the finished run publishes its driver log, got {first:?}"
    );

    // Four more reads of the same run, each of which settles the store again.
    for _ in 0..4 {
        let read = get_job_run(&router, &application_id, &job_run_id).await;
        assert_eq!(
            read.status, 200,
            "case once: GetJobRun expected 200, got {} with {:?}",
            read.status, read.body
        );
    }
    let again = keys_under(&router, &run_prefix).await;
    assert_eq!(
        again, first,
        "case once: re-reading a finished run must publish nothing further, got {again:?}"
    );
}

/// RULE: nothing is published before the run has finished — the artifacts are what the
/// FINISHED run left behind, so a client polling an in-flight run finds the destination
/// empty for it.
#[tokio::test]
async fn an_in_flight_job_run_has_published_nothing_yet() {
    let router = emulator_with_object_store();
    given_bucket(&router).await;

    let created = create_application(
        &router,
        json!({
            "clientToken": "app-inflight",
            "releaseLabel": "emr-7.1.0",
            "type": "SPARK",
            "monitoringConfiguration": monitoring(&format!("s3://{BUCKET}/inflight/")),
        }),
    )
    .await;
    let application_id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let submitted = start_job_run(
        &router,
        &application_id,
        json!({
            "clientToken": "run-inflight",
            "executionRoleArn": EXECUTION_ROLE,
            "jobDriver": spark_driver(),
        }),
    )
    .await;
    let job_run_id = submitted
        .at("/jobRunId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The read that observes a state the run still leaves is a read of an UNFINISHED run,
    // so nothing may have been published for it yet. Whichever state that read sees, the
    // assertion is about the same instant: the listing is taken straight after it.
    let read = get_job_run(&router, &application_id, &job_run_id).await;
    let state = read
        .at("/jobRun/state")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let run_prefix = format!("inflight/applications/{application_id}/jobs/{job_run_id}/");
    if !TERMINAL_STATES.contains(&state.as_str()) {
        let keys = keys_under(&router, &run_prefix).await;
        assert!(
            keys.is_empty(),
            "case in flight: a run reported {state:?} has published nothing yet, got {keys:?}"
        );
    }

    // And once it has finished, the artifact is there: the case is about WHEN, not about
    // whether.
    poll_until_terminal(&router, "in flight", &application_id, &job_run_id).await;
    let keys = poll_until_published(&router, &run_prefix).await;
    assert_eq!(
        keys.len(),
        1,
        "case in flight: the finished run publishes its driver log, got {keys:?}"
    );
}

/// A guard on the shared helper this suite leans on: every row reaches the object store
/// through the SAME router this service is mounted on, so a listing that answers at all
/// proves the two services share one engine.
#[tokio::test]
async fn the_object_store_is_mounted_beside_this_service() {
    let router = emulator_with_object_store();
    given_bucket(&router).await;
    let answer = call(&router, "GET", "/applications", None).await;
    assert_eq!(
        answer.status, 200,
        "case mounted: ListApplications expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let keys = keys_under(&router, "applications/").await;
    assert!(
        keys.is_empty(),
        "case mounted: a fresh bucket holds no artifacts, got {keys:?}"
    );
}
