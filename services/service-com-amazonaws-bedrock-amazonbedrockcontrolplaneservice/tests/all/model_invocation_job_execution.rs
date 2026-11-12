// SPDX-License-Identifier: CC0-1.0

//! What a batch inference job DOES, asserted end to end with this emulator's own Amazon S3
//! mounted beside the control plane on one engine — the way the assembled emulator runs
//! them, and the way the real service takes a job's records on.
//!
//! THE RULE UNDER TEST: a batch inference job is not a status record. Its statuses are tied
//! to real work over real data — `Validating` checks that "your files are .jsonl files and
//! each individual record is a JSON object in the correct format" and that they "fulfill
//! the requirements for file size and number of records", `InProgress` says "this job has
//! begun. You can start viewing the results in the output S3 location", and `Completed`
//! says "view the output files in the output S3 location"
//! (`smithy_query(query="shape",shape="ModelInvocationJobSummary")`, member `status`). So a
//! job this emulator reports as having begun has READ every record of the object its
//! `inputDataConfig` names, has INVOKED the model on each record's `modelInput`, has
//! WRITTEN one result record per input record under the prefix its `outputDataConfig`
//! names, and reports `totalRecordCount` / `processedRecordCount` / `successRecordCount` /
//! `errorRecordCount` that account for them. A job whose input that store itself reports
//! unusable takes the modelled failure path with a `message` instead, and writes nothing.
//!
//! Both halves are asserted over a TABLE whose rows differ along what the rule generalises
//! over: how many records the input holds, whether a record carries its own `recordId`,
//! which model family the records are invoked on (four of them), which invocation endpoint
//! the job names (`InvokeModel` and `Converse`), how the output location is shaped (a
//! prefix, a nested prefix, the bucket itself), whether every record can be invoked (all,
//! some, none), and — for the failure half — which validation criterion the input breaks.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{error_code, label, Reply};

/// The Region both services are addressed in. Amazon S3 places a bucket created with no
/// `LocationConstraint` in us-east-1 ("If you don't specify a Region, the bucket is created
/// in the US East (N. Virginia) Region (us-east-1) by default"), which is also the Region
/// this service serves a request carrying no credential scope in, so one Region label
/// serves every call here.
pub(crate) const REGION: &str = "us-east-1";

/// A SigV4 credential scope naming Amazon S3, which is what routes a request to the object
/// store. No authenticator is registered, so the signature is never checked; the scope is
/// what claims the request.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The bucket every input object below is staged in and every result object is written to.
pub(crate) const BUCKET: &str = "example-batch-inference";

/// A bucket no `CreateBucket` below ever creates.
const ABSENT_BUCKET: &str = "example-batch-inference-missing";

/// How long after it began a job is owed its terminal status.
const COMPLETE_AFTER: Duration = bedrock::state::MODEL_INVOCATION_JOB_COMPLETE_AFTER;

/// The spans the submission arms the progression with, each counted from the one before it,
/// and the instants after the submission they put each stage at.
///
/// They live HERE, beside the harness every batch inference suite drives, so the suites
/// cannot drift from one another when a span changes: each is read off this service's own
/// constants rather than written out as a figure.
const VALIDATES_AFTER: Duration = bedrock::state::MODEL_INVOCATION_JOB_VALIDATES_AFTER;
const SCHEDULES_AFTER: Duration = bedrock::state::MODEL_INVOCATION_JOB_SCHEDULES_AFTER;
const BEGINS_AFTER: Duration = bedrock::state::MODEL_INVOCATION_JOB_BEGINS_AFTER;

/// When `Validating` is owed, counted from the submission.
pub(crate) fn validating_at() -> Duration {
    VALIDATES_AFTER
}

/// When `Scheduled` is owed, counted from the submission.
pub(crate) fn scheduled_at() -> Duration {
    validating_at() + SCHEDULES_AFTER
}

/// When `InProgress` is owed, counted from the submission: the three pre-start spans end to
/// end.
pub(crate) fn in_progress_at() -> Duration {
    scheduled_at() + BEGINS_AFTER
}

/// When the terminal status is owed, counted from the submission: the whole lifespan of a job
/// nobody stops.
pub(crate) fn completed_at() -> Duration {
    in_progress_at() + COMPLETE_AFTER
}

/// Room for a slow machine on top of a clock-owed span.
const SLACK: Duration = Duration::from_millis(500);

/// One stored batch inference job, cloned out of the store the handlers share so a case can
/// settle it at an instant of its own choosing: the span a running job dwells for is longer
/// than the three before it, so a case that only needs the terminal status reads it off a
/// clone rather than sleeping through that span as well.
pub(crate) type JobRecord = bedrock::state::ModelInvocationJobRecord;

/// One emulator: this service and Amazon S3 on a single shared engine behind the production
/// router — what `build_router` assembles, narrowed to the services under test.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

pub(crate) fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(bedrock::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router, engine }
}

impl Emulator {
    /// One control-plane call, signed for [`REGION`] the way an SDK signs it.
    pub(crate) async fn bedrock(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("a serializable request body")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://bedrock.{REGION}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|value| error_code(value).to_string());
        let body: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(|value| error_code(value).to_string())
        });
        Reply {
            status,
            content_type,
            body,
            error,
        }
    }

    /// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
    /// live in the URI and the body is the object's own bytes, so this is a raw request
    /// rather than a modelled call. The credential scope names `s3`, which is what the
    /// router claims on.
    pub(crate) async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, bytes::Bytes) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", S3_AUTHORIZATION)
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }

    /// The stored record of one job, cloned out of the shared store.
    pub(crate) fn stored_job(&self, arn: &str) -> JobRecord {
        let state = bedrock::state::service_state(&*self.engine)
            .expect("the shared engine carries this service's store");
        let store = state.lock();
        store
            .model_invocation_jobs
            .get(arn)
            .unwrap_or_else(|| panic!("the store holds no job at {arn}"))
            .clone()
    }

    /// Age the stored job `arn` names by `by`, as though every instant of its timeline had
    /// fallen that much earlier: the submission, the last change and every instant the clock
    /// owes it a transition at move back together, so the job's own timeline is untouched and
    /// only the amount of it that has already elapsed changes.
    ///
    /// This is how a case brings the wall clock a caller polling at a long interval would
    /// wait through without sleeping through it.
    pub(crate) fn age_job(&self, arn: &str, by: Duration) {
        let state = bedrock::state::service_state(&*self.engine)
            .expect("the shared engine carries this service's store");
        let mut store = state.lock();
        let job = store
            .model_invocation_jobs
            .get_mut(arn)
            .unwrap_or_else(|| panic!("the store holds no job at {arn}"));
        job.submit_time -= by;
        job.last_modified_at -= by;
        if let Some(ended) = job.ended_at.as_mut() {
            *ended -= by;
        }
        if let Some(expires) = job.job_expiration_time.as_mut() {
            *expires -= by;
        }
        for due in [
            &mut job.validating_due_at,
            &mut job.scheduled_due_at,
            &mut job.in_progress_due_at,
            &mut job.completed_due_at,
            &mut job.stopped_due_at,
        ] {
            if let Some(instant) = due.as_mut() {
                *instant -= by;
            }
        }
    }
}

/// `PUT /<bucket>` — `CreateBucket` — so the input objects below have a bucket to live in
/// and the results have one to be written to.
pub(crate) async fn given_bucket(emu: &Emulator) {
    let (status, body) = emu.s3("PUT", &format!("/{BUCKET}"), &[]).await;
    assert_eq!(
        status,
        200,
        "the bucket must be created first, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// `PUT /<bucket>/<key>` — `PutObject` — of a `.jsonl` file holding `lines`, one JSON text
/// per line, which is the documented input format.
pub(crate) async fn given_input_object(emu: &Emulator, key: &str, lines: &[String]) {
    let file: String = lines.iter().map(|line| format!("{line}\n")).collect();
    let (status, body) = emu
        .s3("PUT", &format!("/{BUCKET}/{key}"), file.as_bytes())
        .await;
    assert_eq!(
        status,
        200,
        "the input object {BUCKET}/{key} must be stored, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// `CreateModelInvocationJob` with the members every row sets, answering with the `jobArn`.
pub(crate) async fn submit(
    emu: &Emulator,
    name: &str,
    model_id: &str,
    invocation_type: &str,
    input_uri: &str,
    output_uri: &str,
) -> String {
    let reply = emu
        .bedrock(
            "POST",
            "/model-invocation-job",
            Some(json!({
                "jobName": name,
                "modelId": model_id,
                "modelInvocationType": invocation_type,
                "roleArn": "arn:aws:iam::000000000000:role/BatchInferenceRole",
                "inputDataConfig": {
                    "s3InputDataConfig": {"s3InputFormat": "JSONL", "s3Uri": input_uri}
                },
                "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": output_uri}},
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name}: the submission must be accepted, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("jobArn")
}

/// `GetModelInvocationJob` on the job `arn` names.
pub(crate) async fn read_job(emu: &Emulator, arn: &str) -> Reply {
    emu.bedrock(
        "GET",
        &format!("/model-invocation-job/{}", label(arn)),
        None,
    )
    .await
}

/// The minted job id a `ModelInvocationJobArn` carries, which is the folder a job's results
/// live under.
pub(crate) fn job_id(arn: &str) -> &str {
    arn.rsplit('/').next().expect("an ARN has a last segment")
}

/// How long a case polls a job for before it concludes the progression is stuck.
///
/// An absolute bound, generous next to the spans this service arms — room for a slow machine
/// rather than for another order of magnitude.
pub(crate) const POLL_BUDGET: Duration = Duration::from_secs(45);

/// How long a case waits between two reads of a job while it polls it.
pub(crate) const POLL_STEP: Duration = Duration::from_millis(100);

/// Poll `GetModelInvocationJob` the way a client does until the job reports `target`, and
/// hand back the read that first reported it.
///
/// The progression runs on the submission's own clock, and each stage dwells its own span, so
/// a case reaches a stage by polling the way a client does — which is also what proves the
/// stage is one a poller can read the job in, rather than one the elapsed time carries it
/// past unseen.
pub(crate) async fn poll_until_status(
    emu: &Emulator,
    case: &str,
    arn: &str,
    target: &str,
) -> Reply {
    let started = ::std::time::Instant::now();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reply = read_job(emu, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the job must stay readable while it is polled, got {} {:?}",
            reply.status, reply.body
        );
        let status = reply.text("status");
        if status == target {
            return reply;
        }
        if seen.last() != Some(&status) {
            seen.push(status);
        }
        assert!(
            started.elapsed() < POLL_BUDGET,
            "case {case:?}: a client polling the job must read it {target} inside \
             {POLL_BUDGET:?}, saw {seen:?}"
        );
        ::tokio::time::sleep(POLL_STEP).await;
    }
}

/// The record count a response reports under `name`, or `None` when it omits the member.
pub(crate) fn count(reply: &Reply, name: &str) -> Option<i64> {
    reply.get(name).as_i64()
}

/// One input record in the documented form: a JSON object carrying `modelInput`, and
/// `recordId` when the row gives one.
pub(crate) fn record(record_id: Option<&str>, model_input: Value) -> String {
    let mut line = serde_json::Map::new();
    if let Some(record_id) = record_id {
        line.insert("recordId".to_string(), json!(record_id));
    }
    line.insert("modelInput".to_string(), model_input);
    Value::Object(line).to_string()
}

/// A body `InvokeModel` serves for the Meta Llama family: the family's documented request
/// schema requires `prompt`.
pub(crate) fn llama_input(prompt: &str) -> Value {
    json!({"prompt": prompt, "max_gen_len": 64})
}

/// A body `InvokeModel` serves for the Amazon Titan Text family, whose documented request
/// schema requires `inputText`.
fn titan_input(prompt: &str) -> Value {
    json!({"inputText": prompt})
}

/// A body `InvokeModel` serves for the Anthropic Claude Messages family, whose documented
/// request schema requires `messages`, `max_tokens` and `anthropic_version`.
fn claude_input(prompt: &str) -> Value {
    json!({
        "anthropic_version": "bedrock-2023-05-31",
        "max_tokens": 64,
        "messages": [{"role": "user", "content": prompt}],
    })
}

/// A body the `Converse` endpoint serves: the conversation's `messages`, as that
/// operation's own input binds them.
fn converse_input(prompt: &str) -> Value {
    json!({
        "messages": [{"role": "user", "content": [{"text": prompt}]}],
        "inferenceConfig": {"maxTokens": 128},
    })
}

/// One row of the run table.
struct Row {
    case: &'static str,
    /// `jobName`, which is also the input object's own name.
    name: &'static str,
    /// `modelId`: a model the published catalog carries and the inference surface serves.
    model_id: &'static str,
    /// `modelInvocationType`: the endpoint each record's `modelInput` is a body for.
    invocation_type: &'static str,
    /// The key the input object is staged under.
    input_key: &'static str,
    /// The `outputDataConfig.s3OutputDataConfig.s3Uri` suffix after the bucket: a prefix, a
    /// nested prefix, or nothing at all (the bucket itself).
    output_suffix: &'static str,
    /// The input file's lines, in order.
    lines: Vec<String>,
    /// `successRecordCount` the run must report.
    success: i64,
    /// `errorRecordCount` the run must report.
    error: i64,
    /// The terminal `ModelInvocationJobStatus` the run decides for the job.
    terminal_status: &'static str,
}

/// The run table: what a job does to its records, over inputs differing along record count,
/// `recordId`, model family, invocation endpoint, output location shape and how many of the
/// records the model can be invoked for.
fn run_rows() -> Vec<Row> {
    vec![
        Row {
            case: "three Llama records with their own recordIds, under a prefix",
            name: "batch-llama",
            model_id: "meta.llama3-1-8b-instruct-v1:0",
            invocation_type: "InvokeModel",
            input_key: "input/llama.jsonl",
            output_suffix: "/output",
            lines: vec![
                record(Some("RECORD00001"), llama_input("one")),
                record(Some("RECORD00002"), llama_input("two")),
                record(Some("RECORD00003"), llama_input("three")),
            ],
            success: 3,
            error: 0,
            terminal_status: "Completed",
        },
        Row {
            case: "one Titan Text record carrying no recordId, under a nested prefix",
            name: "batch-titan",
            model_id: "amazon.titan-text-lite-v1",
            invocation_type: "InvokeModel",
            input_key: "input/titan.jsonl",
            output_suffix: "/output/runs/titan",
            lines: vec![record(None, titan_input("summarise the quarter"))],
            success: 1,
            error: 0,
            terminal_status: "Completed",
        },
        Row {
            case: "two Claude records, one of which omits a member its family's request \
                   schema requires, with the results at the bucket root",
            name: "batch-claude",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            invocation_type: "InvokeModel",
            input_key: "input/claude.jsonl",
            output_suffix: "",
            lines: vec![
                record(Some("RECORD00001"), claude_input("hello")),
                record(Some("RECORD00002"), json!({"messages": []})),
            ],
            success: 1,
            error: 1,
            terminal_status: "PartiallyCompleted",
        },
        Row {
            case: "two records invoked through the Converse endpoint",
            name: "batch-converse",
            model_id: "anthropic.claude-3-haiku-20240307-v1:0",
            invocation_type: "Converse",
            input_key: "input/converse.jsonl",
            output_suffix: "/output",
            lines: vec![
                record(Some("RECORD00001"), converse_input("hello")),
                record(None, converse_input("and again")),
            ],
            success: 2,
            error: 0,
            terminal_status: "Completed",
        },
        Row {
            case: "a Converse job against a model the Converse API does not serve, so no \
                   record of it can be invoked",
            name: "batch-converse-unserved",
            model_id: "cohere.command-text-v14",
            invocation_type: "Converse",
            input_key: "input/cohere.jsonl",
            output_suffix: "/output",
            lines: vec![record(Some("RECORD00001"), converse_input("hello"))],
            success: 0,
            error: 1,
            terminal_status: "Failed",
        },
    ]
}

/// A job that has begun has read its records, invoked the model over each one, written one
/// result record per input record under its output prefix and reports counts that account
/// for them.
#[tokio::test]
async fn a_job_that_has_begun_has_run_the_model_over_its_input_and_written_its_results() {
    let emu = emulator();
    given_bucket(&emu).await;

    let rows = run_rows();
    let mut submitted: Vec<(usize, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        given_input_object(&emu, row.input_key, &row.lines).await;
        let arn = submit(
            &emu,
            row.name,
            row.model_id,
            row.invocation_type,
            &format!("s3://{BUCKET}/{}", row.input_key),
            &format!("s3://{BUCKET}{}", row.output_suffix),
        )
        .await;
        submitted.push((index, arn));
    }

    // Every row is polled to the status that says it has begun: a job leaves each stage of
    // the progression no earlier than the first read that finds it there, so the status is
    // reached by reading the job rather than by sleeping through the pre-start spans.
    for (index, arn) in &submitted {
        let row = &rows[*index];
        let case = row.case;
        let reply = poll_until_status(&emu, case, arn, "InProgress").await;

        let total = i64::try_from(row.lines.len()).expect("a small record count");
        for (member, expected) in [
            ("totalRecordCount", total),
            ("processedRecordCount", row.success + row.error),
            ("successRecordCount", row.success),
            ("errorRecordCount", row.error),
        ] {
            let actual = count(&reply, member);
            assert_eq!(
                actual,
                Some(expected),
                "case {case:?}: {member} must be {expected}, got {actual:?}"
            );
        }

        // The results, at the location the job was told to write them: the prefix the
        // submission named, a folder named for the job, and the input file's own name.
        let input_name = row
            .input_key
            .rsplit('/')
            .next()
            .expect("the key has a last segment");
        let key = format!(
            "{}{}/{input_name}.out",
            row.output_suffix.trim_start_matches('/'),
            format_args!(
                "{}{}",
                if row.output_suffix.is_empty() { "" } else { "/" },
                job_id(arn)
            ),
        );
        let (status, body) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the results must be stored at {BUCKET}/{key}, got {status} with \
             body {:?}",
            String::from_utf8_lossy(&body)
        );
        assert!(
            !body.is_empty(),
            "case {case:?}: the result object at {BUCKET}/{key} must not be empty"
        );

        let text = String::from_utf8(body.to_vec()).expect("the results are UTF-8 text");
        let results: Vec<Value> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("case {case:?}: result line {line:?} is not JSON: {error}")
                })
            })
            .collect();
        assert_eq!(
            results.len(),
            row.lines.len(),
            "case {case:?}: one result record per input record, expected {}, got {}",
            row.lines.len(),
            results.len()
        );

        let mut served = 0i64;
        let mut failed = 0i64;
        for (position, result) in results.iter().enumerate() {
            let record_id = result.get("recordId").and_then(Value::as_str);
            assert!(
                record_id.is_some_and(|id| !id.is_empty()),
                "case {case:?}: result {position} must carry a recordId, got {result:?}"
            );
            let model_input = result.get("modelInput");
            assert!(
                model_input.is_some(),
                "case {case:?}: result {position} must carry its modelInput, got {result:?}"
            );
            match result.get("modelOutput") {
                Some(model_output) => {
                    served += 1;
                    assert_ne!(
                        Some(model_output),
                        model_input,
                        "case {case:?}: result {position} must carry what the model answered, \
                         not its own input echoed back"
                    );
                    assert!(
                        model_output.is_object(),
                        "case {case:?}: result {position} must carry the model's own response \
                         envelope, got {model_output:?}"
                    );
                }
                None => {
                    failed += 1;
                    let message = result
                        .get("error")
                        .and_then(|error| error.get("errorMessage"))
                        .and_then(Value::as_str);
                    assert!(
                        message.is_some_and(|message| !message.is_empty()),
                        "case {case:?}: result {position} carries no modelOutput, so it must \
                         say why, got {result:?}"
                    );
                }
            }
        }
        assert_eq!(
            (served, failed),
            (row.success, row.error),
            "case {case:?}: the stored results must account for the reported counts, \
             expected {:?}, got {:?}",
            (row.success, row.error),
            (served, failed)
        );

        // The terminal status the run decided, read off the record settled past the instant
        // the job is owed it, rather than by sleeping through the running span once more. The
        // job was READ running above, so the only transition it still owes is that terminal
        // one, a run span after that read.
        let mut record = emu.stored_job(arn);
        record.settle(SystemTime::now() + COMPLETE_AFTER + SLACK);
        assert_eq!(
            record.status, row.terminal_status,
            "case {case:?}: the job must end {}, got {}",
            row.terminal_status, record.status
        );
        assert!(
            record.ended_at.is_some(),
            "case {case:?}: a job that has ended reports when it ended"
        );
        let message_expected = row.success == 0;
        assert_eq!(
            record.message.is_some(),
            message_expected,
            "case {case:?}: a job no record of which was processed reports why, got {:?}",
            record.message
        );
    }
}

/// One row of the validation table: an input the object store itself reports unusable.
struct FailureRow {
    case: &'static str,
    name: &'static str,
    /// The input object staged for this row, when the row stages one at all.
    staged: Option<(&'static str, &'static str)>,
    /// The `inputDataConfig.s3InputDataConfig.s3Uri` the submission carries.
    input_uri: String,
}

/// An input the store itself reports unusable fails the JOB — with a message, no results and
/// no record counts — rather than reporting a job that did nothing as completed.
#[tokio::test]
async fn an_input_the_object_store_reports_unusable_fails_the_job() {
    let emu = emulator();
    given_bucket(&emu).await;

    // The rows differ along which validation criterion the input breaks: the object is not
    // there, its bucket is not there, the URI names no object at all, a line is not a JSON
    // object, a record binds no `modelInput`, and the file holds no records.
    let rows = [
        FailureRow {
            case: "a key the bucket does not hold",
            name: "batch-missing-key",
            staged: None,
            input_uri: format!("s3://{BUCKET}/input/never-uploaded.jsonl"),
        },
        FailureRow {
            case: "a bucket that was never created",
            name: "batch-missing-bucket",
            staged: None,
            input_uri: format!("s3://{ABSENT_BUCKET}/input/records.jsonl"),
        },
        FailureRow {
            case: "a URI naming the bucket rather than an object",
            name: "batch-no-object",
            staged: None,
            input_uri: format!("s3://{BUCKET}"),
        },
        FailureRow {
            case: "a line that is not a JSON object",
            name: "batch-not-json",
            staged: Some(("input/not-json.jsonl", "this is not a record\n")),
            input_uri: format!("s3://{BUCKET}/input/not-json.jsonl"),
        },
        FailureRow {
            case: "a record that binds no modelInput",
            name: "batch-no-model-input",
            staged: Some((
                "input/no-model-input.jsonl",
                "{\"recordId\":\"RECORD00001\"}\n",
            )),
            input_uri: format!("s3://{BUCKET}/input/no-model-input.jsonl"),
        },
        FailureRow {
            case: "a file holding no records",
            name: "batch-empty",
            staged: Some(("input/empty.jsonl", "\n\n")),
            input_uri: format!("s3://{BUCKET}/input/empty.jsonl"),
        },
    ];

    let mut submitted: Vec<(usize, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if let Some((key, text)) = row.staged {
            let (status, body) = emu
                .s3("PUT", &format!("/{BUCKET}/{key}"), text.as_bytes())
                .await;
            assert_eq!(
                status,
                200,
                "case {:?}: the input object must be stored, got {status} with body {:?}",
                row.case,
                String::from_utf8_lossy(&body)
            );
        }
        let arn = submit(
            &emu,
            row.name,
            "meta.llama3-1-8b-instruct-v1:0",
            "InvokeModel",
            &row.input_uri,
            &format!("s3://{BUCKET}/output"),
        )
        .await;
        submitted.push((index, arn));
    }

    // A job whose input cannot be processed is validated and then failed, one span apart, so
    // a client polling it reads it queued, then being validated, then failed.
    for (index, arn) in &submitted {
        let case = rows[*index].case;
        let reply = poll_until_status(&emu, case, arn, "Failed").await;
        assert_eq!(
            reply.get("status"),
            json!("Failed"),
            "case {case:?}: the job must fail over its input, got {:?}",
            reply.get("status")
        );
        let message = reply.get("message");
        assert!(
            message.as_str().is_some_and(|message| !message.is_empty()),
            "case {case:?}: a failed job says why it failed, got {message:?}"
        );
        assert!(
            reply.get("endTime").is_string(),
            "case {case:?}: a job that has ended reports when it ended, got {:?}",
            reply.get("endTime")
        );
        for member in [
            "totalRecordCount",
            "processedRecordCount",
            "successRecordCount",
            "errorRecordCount",
        ] {
            assert_eq!(
                count(&reply, member),
                None,
                "case {case:?}: no records were counted, so {member} is reported by no count"
            );
        }

        // Nothing was written: a job that could not read its input produced no results.
        let key = format!("output/{}/records.jsonl.out", job_id(arn));
        let (status, _) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a failed job writes nothing under its output prefix, got {status} \
             for {BUCKET}/{key}"
        );
    }
}

/// The counts a LISTING reports are the counts the read of that one job reports: both are
/// the same run's tally, and a listing takes a run its jobs are owed exactly as a read does.
#[tokio::test]
async fn a_listing_reports_the_counts_the_read_of_one_job_reports() {
    let emu = emulator();
    given_bucket(&emu).await;

    let lines = vec![
        record(Some("RECORD00001"), llama_input("one")),
        record(Some("RECORD00002"), llama_input("two")),
    ];
    given_input_object(&emu, "input/listed.jsonl", &lines).await;
    let arn = submit(
        &emu,
        "batch-listed",
        "meta.llama3-1-8b-instruct-v1:0",
        "InvokeModel",
        &format!("s3://{BUCKET}/input/listed.jsonl"),
        &format!("s3://{BUCKET}/output"),
    )
    .await;

    // Before the job has begun it owes its run, so nothing has been processed yet and the
    // listing reports exactly the records validation counted.
    let listed = emu.bedrock("GET", "/model-invocation-jobs", None).await;
    let summaries = listed.get("invocationJobSummaries");
    let summary = summaries
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .unwrap_or_else(|| panic!("the listing must carry the submitted job, got {summaries:?}"));
    assert_eq!(
        summary.get("totalRecordCount").and_then(Value::as_i64),
        Some(2),
        "the records validation counted are reported from the submission onwards, got \
         {summary:?}"
    );
    assert_eq!(
        summary.get("processedRecordCount").and_then(Value::as_i64),
        Some(0),
        "a job that has not begun has processed nothing, got {summary:?}"
    );

    // A LISTING observes every job it carries exactly as a read observes one, so the
    // listing is what carries this job through its progression here — and it is what takes
    // the run, so the counts it reports are the run's own.
    let started = ::std::time::Instant::now();
    let summary = loop {
        let listed = emu.bedrock("GET", "/model-invocation-jobs", None).await;
        let summary = listed
            .get("invocationJobSummaries")
            .as_array()
            .and_then(|rows| rows.first())
            .cloned()
            .expect("the listing carries the submitted job");
        if summary.get("status").and_then(Value::as_str) == Some("InProgress") {
            break summary;
        }
        assert!(
            started.elapsed() < POLL_BUDGET,
            "a client listing the jobs must read this one running inside {POLL_BUDGET:?}, got \
             {summary:?}"
        );
        ::tokio::time::sleep(POLL_STEP).await;
    };
    let read = read_job(&emu, &arn).await;
    for (member, expected) in [
        ("totalRecordCount", 2),
        ("processedRecordCount", 2),
        ("successRecordCount", 2),
        ("errorRecordCount", 0),
    ] {
        let listed_count = summary.get(member).and_then(Value::as_i64);
        assert_eq!(
            listed_count,
            Some(expected),
            "{member}: the listing must report the run's own tally, expected {expected}, got \
             {listed_count:?}"
        );
        assert_eq!(
            listed_count,
            count(&read, member),
            "{member}: a listing and a read of one job must agree, got {listed_count:?} and \
             {:?}",
            count(&read, member)
        );
    }

    // The results were stored once: a second read reports the same counts and leaves what is
    // stored under the output prefix as it was.
    let key = format!("output/{}/listed.jsonl.out", job_id(&arn));
    let (status, first) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
    assert_eq!(status, 200, "the results must be stored at {BUCKET}/{key}");
    let _ = read_job(&emu, &arn).await;
    let (status, second) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
    assert_eq!(status, 200, "the results must still be stored");
    assert_eq!(
        first, second,
        "a later read must not rewrite what a client already fetched"
    );
}

/// A job submitted with no object store to ask keeps the lifecycle it has always had: a
/// verdict about its input has to be GROUNDED in what the store answered, so an emulator
/// assembled without one fails nothing and reports no count.
#[tokio::test]
async fn with_no_object_store_to_ask_nothing_is_concluded_about_the_input() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(bedrock::EmulatedService::new(Arc::clone(&engine)));
    let emu = Emulator { router, engine };

    let arn = submit(
        &emu,
        "batch-unknown-input",
        "meta.llama3-1-8b-instruct-v1:0",
        "InvokeModel",
        &format!("s3://{BUCKET}/input/records.jsonl"),
        &format!("s3://{BUCKET}/output"),
    )
    .await;

    let reply = poll_until_status(&emu, "an unknown input", &arn, "InProgress").await;
    assert_eq!(
        reply.get("status"),
        json!("InProgress"),
        "with nothing known about the input the job keeps its lifecycle, got {:?}",
        reply.get("status")
    );
    assert_eq!(
        reply.get("message"),
        Value::Null,
        "nothing was established about the input, so nothing failed"
    );
    for member in [
        "totalRecordCount",
        "processedRecordCount",
        "successRecordCount",
        "errorRecordCount",
    ] {
        assert_eq!(
            count(&reply, member),
            None,
            "{member}: no records were counted, so it is reported by no count"
        );
    }

    // The job was READ running, so the only transition it still owes is its terminal status,
    // a run span after that read.
    let mut record = emu.stored_job(&arn);
    record.settle(SystemTime::now() + COMPLETE_AFTER + SLACK);
    assert_eq!(
        record.status, "Completed",
        "the lifecycle a job with an unknown input runs is the one it has always had, got {}",
        record.status
    );
    assert!(
        SystemTime::now() > record.submit_time,
        "the submission is stamped with the clock it was served on"
    );
}
