// SPDX-License-Identifier: CC0-1.0

//! Running a submitted S3 Batch Operations job: its object list, its tasks and
//! the objects it leaves behind.
//!
//! `CreateJob` "creates an S3 Batch Operations job" that "performs a specified
//! operation on every object listed in the manifest" — the call ANSWERS with the
//! job's id while the job is still owed, so the work runs here, off the response
//! path, and the job's status is what a caller polls (`DescribeJob`) to learn it
//! is done. A job walks the statuses the model declares: `New` while the clock
//! owes its preparation, `Ready` once prepared, [`JOB_STATUS_ACTIVE`] while its
//! tasks are being run, and then the terminal [`JOB_STATUS_COMPLETE`] — however
//! many of its individual tasks failed, which `ProgressSummary` accounts for —
//! or [`JOB_STATUS_FAILED`] for a job that could not be run at all, with
//! `FailureReasons` saying why.
//!
//! The work is carried out AS the role the create named ("the IAM role that
//! Batch Operations will use to run this job's operation on every object in the
//! manifest", `JobDescriptor$RoleArn`), so that role is assumed BEFORE the first
//! task: a job whose role the account's IAM holds a trust document for that
//! withholds the assume from this service's principal settles
//! [`JOB_STATUS_FAILED`] with that reason and leaves every object of its list
//! untouched.
//!
//! Every task is carried out through the OWNING service's own API: the objects a
//! copy writes, the tags a tagging task sets, the generated manifest and the
//! completion report are Amazon S3 objects, written by Amazon S3's own
//! operations over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]) — so a client that
//! waits for the job's terminal status and then reads those objects through
//! Amazon S3 finds exactly what the job did, and nothing here opens a socket or
//! leaves the host. The one caller-supplied value that reaches the client's
//! authority is the signing Region, narrowed before it gets there
//! ([`service_common::arn::region_label`]).
//!
//! The store's guard is never held across a call to Amazon S3: the submitted job
//! is read under it, the tasks are run with it released, and it is taken again to
//! record what each task did. A job a caller cancelled while the run was under
//! way is left as the cancellation left it.
//!
//! The work one job does is BOUNDED: at most [`MAX_JOB_TASKS`] tasks, over a
//! manifest of at most that many rows or a source bucket listing of at most
//! [`MAX_SOURCE_OBJECTS`] objects, each task one request. There is no retry and
//! no loop over the clock: the worker waits once, runs each task once, records
//! the outcome and ends.

use std::sync::Arc;

use ::aws_smithy_runtime_api::client::result::SdkError;
use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;
use ::smithy_server_runtime::EmbeddedEmulator;

use super::batch_job_plan::{
    arn_bucket, arn_bucket_and_key, generated_manifest_csv, generated_manifest_format,
    inventory_manifest_json, job_prefix, parse_csv_manifest, prefixed_key, report_manifest_json,
    report_results_csv, selected_by_filter, written_manifest_location, ManifestEntry, TaskOutcome,
    MANIFEST_FORMAT_CSV, TASK_FAILED, TASK_SUCCEEDED, TASK_SUCCEEDED_MESSAGE,
};
use super::*;

/// The one reading of whether S3 Batch Operations can assume the role a job's
/// tasks run as ([`super::batch_job_role`]).
use super::batch_job_role::engine_assume_refusal;

use crate::ops::types::{
    JobManifestGenerator, S3CopyObjectOperation, S3GeneratedManifestDescriptor,
    S3JobManifestGenerator,
};

/// How many objects of a source bucket a manifest GENERATOR enumerates before
/// the job is failed rather than run over part of the bucket.
///
/// The generator selects its object list from "the source bucket used by the
/// `ManifestGenerator`" (`S3JobManifestGenerator$SourceBucket`), which this
/// emulator holds in memory: this is its OWN bound on how much of one bucket a
/// single job walks, set far above any workflow an emulator serves and well
/// above [`MAX_JOB_TASKS`], so a filter that selects nothing from a large bucket
/// is still answered rather than silently truncated.
pub const MAX_SOURCE_OBJECTS: usize = 10_000;

/// How many keys one page of the source bucket's listing asks for.
const LISTING_PAGE_SIZE: i32 = 1_000;

/// How many pages of a source bucket's listing one job walks: enough for
/// [`MAX_SOURCE_OBJECTS`] keys at [`LISTING_PAGE_SIZE`] a page, which is also
/// what stops the walk when a listing keeps handing back a continuation token.
const MAX_LISTING_PAGES: usize = MAX_SOURCE_OBJECTS / (LISTING_PAGE_SIZE as usize) + 1;

/// The key the data file of the manifest a generator WRITES is stored at, under
/// the job's own prefix ([`job_prefix`]) in the manifest output location.
const GENERATED_MANIFEST_NAME: &str = "manifest.csv";

/// The key the manifest OF that data file is stored at, beside it: a manifest in
/// the generated format is read from this object (see
/// [`super::batch_job_plan::inventory_manifest_json`]).
const GENERATED_MANIFEST_MANIFEST_NAME: &str = "manifest.json";

/// The key a completion report's own manifest is stored at, under the job's
/// prefix in the report bucket: the one fixed key a reader of the report starts
/// from, which names the result file.
const REPORT_MANIFEST_NAME: &str = "manifest.json";

/// Where a completion report's result file is stored, under the job's prefix.
const REPORT_RESULTS_NAME: &str = "results/report.csv";

/// The `FailureCode` of a job this emulator could not run — `JobFailure$
/// FailureCode` is "the failure code, if any, for the specified job", an
/// unconstrained string in the model, so this is the one code this service
/// answers with beside the reason that says what stopped the job.
const JOB_FAILURE_CODE: &str = "JobFailed";

/// The `FailureCode` of a job whose ROLE this service could not assume: the
/// identity the job's work would have run under refused it, which is what
/// `AccessDenied` says.
const ASSUME_ROLE_FAILURE_CODE: &str = "AccessDenied";

/// The longest `FailureCode` one failure can carry. Source: the
/// `@length(min: 1, max: 64)` of `JobFailureCode` in the Smithy model
/// (`smithy_query(query="shape", shape="JobFailure")`).
pub(super) const FAILURE_CODE_LENGTH_MAX: usize = 64;

/// The longest `FailureReason` one failure can carry. Source: the
/// `@length(min: 1, max: 256)` of `JobFailureReason`, same shape. A reason built
/// from values a request named (a role ARN is `@length(max: 2048)`) is cut to it
/// by [`job_failure`], so every failure this run records is a legal value of the
/// member that carries it — and a reason COMPOSED from such a value is composed
/// to fit it ([`super::batch_job_role::assume_failure_reason`]), so the cut is
/// the last resort rather than the usual case.
pub(super) const FAILURE_REASON_LENGTH_MAX: usize = 256;

/// Run the job the `CreateJob` that returns this accepted.
///
/// Returns at once: the job runs in a task of its own, because the create
/// answers with the job's id while the job is still owed. The caller hands over
/// its own clone of the in-process callback, so the objects the job reads and
/// writes are the ones in the emulator the submitting call was made to.
pub fn start<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    region: String,
    account_id: String,
    job_id: String,
) {
    ::tokio::spawn(async move {
        run(&engine, &callback, &region, &account_id, &job_id).await;
    });
}

/// Wait the run's span out, then carry the job out.
async fn run<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    region: &str,
    account_id: &str,
    job_id: &str,
) {
    ::tokio::time::sleep(JOB_RUN_AFTER).await;

    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Some(submitted) = ({
        let mut store = state.lock();
        store.materialize_due_jobs(SystemTime::now());
        store
            .job(account_id, job_id)
            .filter(|job| runnable(&job.status))
            .map(Submitted::of)
    }) else {
        // A job a caller cancelled, one still awaiting its confirmation, or one
        // the store no longer holds has no run owed to it.
        return;
    };

    // "The Amazon Resource Name (ARN) for the Identity and Access Management
    // (IAM) role that Batch Operations will use to run this job's operation on
    // every object in the manifest" (`JobDescriptor$RoleArn`): the work is
    // carried out AS that role, so the role is assumed BEFORE any task runs,
    // and a role whose trust policy withholds the assume from this service's
    // principal is a job that cannot run at all — it settles failed with that
    // reason, accounting for the nothing it ran, and no object of its list is
    // touched.
    if let Some(reason) = engine_assume_refusal(&**engine, &submitted.role_arn) {
        return fail_with(
            &state,
            account_id,
            job_id,
            ASSUME_ROLE_FAILURE_CODE,
            &reason,
            Some(0),
        );
    }

    let client = s3_client(callback, region);
    let action = match action_of(&submitted.operation) {
        Ok(action) => action,
        Err(reason) => return fail(&state, account_id, job_id, &reason),
    };
    let listed = match object_list(&client, &submitted, job_id).await {
        Ok(listed) => listed,
        Err(reason) => return fail(&state, account_id, job_id, &reason),
    };

    // The object list is what the job's tasks are, so this is where the job
    // starts running them and where its `ProgressSummary` gets a total to
    // account against.
    let started_at = SystemTime::now();
    {
        let mut store = state.lock();
        let Some(job) = store.job_mut(account_id, job_id) else {
            return;
        };
        if !runnable(&job.status) {
            return;
        }
        job.status = job_status(JOB_STATUS_ACTIVE);
        job.status_due_at = None;
        job.total_tasks = Some(i64::try_from(listed.entries.len()).unwrap_or(i64::MAX));
        job.generated_manifest = listed.generated.clone();
    }

    let mut outcomes = Vec::with_capacity(listed.entries.len());
    for entry in &listed.entries {
        let outcome = run_task(&client, &action, entry).await;
        let mut store = state.lock();
        let Some(job) = store.job_mut(account_id, job_id) else {
            return;
        };
        if job_terminal(&job.status) {
            // A cancellation reached the job while its tasks were running: what
            // it stopped stays stopped, and the tasks already carried out stay
            // accounted for.
            return;
        }
        if outcome.succeeded() {
            job.tasks_succeeded += 1;
        } else {
            job.tasks_failed += 1;
        }
        outcomes.push(outcome);
    }

    // "Indicates whether the specified job will generate a job-completion
    // report" (`JobReport$Enabled`): a job asked for one has it written before
    // it reports a terminal status, so a client that waits for the status finds
    // the report there.
    let report_fault = if submitted.report.enabled {
        write_completion_report(&client, &submitted.report, job_id, &outcomes)
            .await
            .err()
    } else {
        None
    };

    let mut store = state.lock();
    let Some(job) = store.job_mut(account_id, job_id) else {
        return;
    };
    if job_terminal(&job.status) {
        return;
    }
    job.elapsed_active_seconds = Some(elapsed_seconds(started_at));
    job.termination_date = Some(::aws_smithy_types::DateTime::from(SystemTime::now()));
    match report_fault {
        // The job ran its tasks, so it is `Complete` however many of them
        // failed: a task failure is what `ProgressSummary` accounts for.
        None => job.status = job_status(JOB_STATUS_COMPLETE),
        // The job was asked for a report and the report could not be written,
        // so the job did not do all it was created to do.
        Some(reason) => {
            job.status = job_status(JOB_STATUS_FAILED);
            job.failures.push(job_failure(JOB_FAILURE_CODE, &reason));
        }
    }
}

/// Whether a job in `status` still has its run owed to it: one that has not
/// ended, is not waiting for a confirmation and has not been paused.
///
/// A job the clock has already carried to [`JOB_STATUS_ACTIVE`] is one of them —
/// that is this run, having already started — which is what keeps a second read
/// of the status from abandoning a run in flight.
fn runnable(status: &str) -> bool {
    !job_terminal(status) && status != JOB_STATUS_SUSPENDED && status != JOB_STATUS_PAUSED
}

/// How many seconds have passed since `started_at`, for
/// `JobTimers$ElapsedTimeInActiveSeconds`.
fn elapsed_seconds(started_at: SystemTime) -> i64 {
    SystemTime::now()
        .duration_since(started_at)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0))
}

/// Record that the job could not be run, with the reason that stopped it:
/// `FailureReasons` is "the reasons for the failure of the specified job", so
/// the job reaches [`JOB_STATUS_FAILED`] carrying exactly what this run could
/// not do.
fn fail(
    state: &::engine::ServiceState<ComAmazonawsS3controlAwss3controlservicev20180820State>,
    account_id: &str,
    job_id: &str,
    reason: &str,
) {
    fail_with(state, account_id, job_id, JOB_FAILURE_CODE, reason, None);
}

/// [`fail`] with the `FailureCode` `code` rather than this run's default one and
/// with `tasks` accounted for in `ProgressSummary`, for a failure of a KIND the
/// service names: a job whose role could not be assumed is refused by an
/// identity before its object list is ever read, so its run accounts for the
/// tasks it carried out — none.
fn fail_with(
    state: &::engine::ServiceState<ComAmazonawsS3controlAwss3controlservicev20180820State>,
    account_id: &str,
    job_id: &str,
    code: &str,
    reason: &str,
    tasks: Option<i64>,
) {
    let mut store = state.lock();
    let Some(job) = store.job_mut(account_id, job_id) else {
        return;
    };
    if job_terminal(&job.status) {
        return;
    }
    job.status = job_status(JOB_STATUS_FAILED);
    job.status_due_at = None;
    job.termination_date = Some(::aws_smithy_types::DateTime::from(SystemTime::now()));
    if let Some(tasks) = tasks {
        job.total_tasks = Some(tasks);
    }
    job.failures.push(job_failure(code, reason));
}

/// One `JobFailure` as `FailureReasons` reports it, with both of its members
/// inside the lengths the model declares for them
/// ([`FAILURE_CODE_LENGTH_MAX`], [`FAILURE_REASON_LENGTH_MAX`]).
///
/// Every failure this run records is built here, so a reason built from values a
/// request named is a legal value of the member however long those values are,
/// and no write of that member carries a bound of its own.
fn job_failure(code: &str, reason: &str) -> JobFailure {
    JobFailure {
        failure_code: Some(within(code, FAILURE_CODE_LENGTH_MAX)),
        failure_reason: Some(within(reason, FAILURE_REASON_LENGTH_MAX)),
    }
}

/// `value` cut to at most `characters` characters, on a character boundary.
pub(super) fn within(value: &str, characters: usize) -> String {
    match value.char_indices().nth(characters) {
        None => value.to_string(),
        Some((cut, _)) => value[..cut].to_string(),
    }
}

/// What a submitted job asked for, copied out from under the store's guard so
/// the run holds no lock while it works.
#[derive(Debug, Clone)]
struct Submitted {
    /// The one action the job runs on every object of its list.
    operation: JobOperation,
    /// The role the job's work is carried out as.
    role_arn: String,
    /// The manifest the create named, if it named one rather than a generator.
    manifest: Option<JobManifest>,
    /// The generator the create named, if it named one rather than a manifest.
    manifest_generator: Option<JobManifestGenerator>,
    /// The completion report the create asked for.
    report: JobReport,
}

impl Submitted {
    /// What `job` asked for.
    fn of(job: &JobRecord) -> Self {
        Self {
            operation: job.operation.clone(),
            role_arn: job.role_arn.clone(),
            manifest: job.manifest.clone(),
            manifest_generator: job.manifest_generator.clone(),
            report: job.report.clone(),
        }
    }
}

/// The object list a job runs over, and the manifest its generator wrote out
/// where it wrote one.
#[derive(Debug, Clone)]
struct Listed {
    /// One entry per task, in the order the manifest or the listing gave them.
    entries: Vec<ManifestEntry>,
    /// The manifest this run WROTE, for a generator asked to write one out.
    generated: Option<S3GeneratedManifestDescriptor>,
}

/// The one action `operation` names, or the reason a job naming it cannot be
/// run.
///
/// The actions this emulator carries out are the ones that ARE a request to
/// Amazon S3 about one object, which it makes through Amazon S3's own API. An
/// action it does not carry out fails the job naming that action, rather than
/// leaving a job reported as having run work nothing did.
#[derive(Debug, Clone)]
enum Action {
    /// `S3PutObjectCopy`: "S3 Batch Operations passes every object to the
    /// underlying `CopyObject` API operation."
    Copy(Box<S3CopyObjectOperation>),
    /// `S3PutObjectTagging`: the tag set to put on every object of the list.
    PutTagging(Vec<S3Tag>),
    /// `S3DeleteObjectTagging`: the tag set of every object of the list is
    /// removed.
    DeleteTagging,
}

/// See [`Action`].
fn action_of(operation: &JobOperation) -> Result<Action, String> {
    if let Some(copy) = operation.s3_put_object_copy.as_ref() {
        return Ok(Action::Copy(Box::new(copy.clone())));
    }
    if let Some(tagging) = operation.s3_put_object_tagging.as_ref() {
        return Ok(Action::PutTagging(
            tagging.tag_set.clone().unwrap_or_default(),
        ));
    }
    if operation.s3_delete_object_tagging.is_some() {
        return Ok(Action::DeleteTagging);
    }
    // `CreateJob` refuses a job naming no single action, so the job carries
    // exactly one and the member names it.
    let named = job_operation_names(operation).join(", ");
    Err(format!(
        "This emulator does not run the {named} action of an S3 Batch Operations job."
    ))
}

/// The object list the job runs over: the rows of the manifest the create
/// named, or the objects of the source bucket its generator selects — written
/// out as a manifest object when the generator asks for one.
async fn object_list(
    client: &::aws_sdk_s3::Client,
    submitted: &Submitted,
    job_id: &str,
) -> Result<Listed, String> {
    if let Some(manifest) = submitted.manifest.as_ref() {
        return Ok(Listed {
            entries: manifest_entries(client, manifest).await?,
            generated: None,
        });
    }
    let Some(JobManifestGenerator::S3JobManifestGenerator(generator)) =
        submitted.manifest_generator.as_ref()
    else {
        // `CreateJob` refuses a job naming neither a manifest nor a generator.
        return Err("The job names no object list to run over.".to_string());
    };
    generated_list(client, generator, job_id).await
}

/// The rows of the manifest OBJECT the create named, read through Amazon S3.
async fn manifest_entries(
    client: &::aws_sdk_s3::Client,
    manifest: &JobManifest,
) -> Result<Vec<ManifestEntry>, String> {
    if manifest.spec.format != MANIFEST_FORMAT_CSV {
        return Err(format!(
            "This emulator reads a job manifest in the {MANIFEST_FORMAT_CSV} format; this one is \
             in {}.",
            manifest.spec.format
        ));
    }
    let Some((bucket, key)) = arn_bucket_and_key(&manifest.location.object_arn) else {
        return Err(format!(
            "The manifest's ObjectArn names no object: {}",
            manifest.location.object_arn
        ));
    };
    let answer = client
        .get_object()
        .bucket(&bucket)
        .key(&key)
        .set_version_id(manifest.location.object_version_id.clone())
        .send()
        .await
        .map_err(|error| {
            format!(
                "The manifest object could not be read: {}",
                refused_by_s3(&error).message
            )
        })?;
    let body = answer
        .body
        .collect()
        .await
        .map_err(|error| format!("The manifest object could not be read: {error}"))?
        .into_bytes();
    parse_csv_manifest(&body, manifest.spec.fields.as_deref().unwrap_or_default())
}

/// The object list a manifest GENERATOR selects from its source bucket, and the
/// manifest object it wrote when `EnableManifestOutput` asked for one.
async fn generated_list(
    client: &::aws_sdk_s3::Client,
    generator: &S3JobManifestGenerator,
    job_id: &str,
) -> Result<Listed, String> {
    let not_applied = super::batch_job_plan::filter_members_not_applied(generator.filter.as_ref());
    if !not_applied.is_empty() {
        return Err(format!(
            "This emulator cannot select a manifest by the generator's {} filter.",
            not_applied.join(", ")
        ));
    }
    let Some(source) = arn_bucket(&generator.source_bucket) else {
        return Err(format!(
            "The generator's SourceBucket names no bucket: {}",
            generator.source_bucket
        ));
    };

    let mut entries: Vec<ManifestEntry> = Vec::new();
    let mut listed = 0_usize;
    let mut pages = 0_usize;
    let mut continuation: Option<String> = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(&source)
            .max_keys(LISTING_PAGE_SIZE)
            .set_continuation_token(continuation.clone())
            .send()
            .await
            .map_err(|error| {
                format!(
                    "The generator's source bucket could not be listed: {}",
                    refused_by_s3(&error).message
                )
            })?;
        for object in page.contents() {
            let Some(key) = object.key() else {
                continue;
            };
            listed += 1;
            if listed > MAX_SOURCE_OBJECTS {
                return Err(format!(
                    "This emulator generates a manifest over at most {MAX_SOURCE_OBJECTS} objects \
                     of a source bucket; {source} holds more."
                ));
            }
            if !selected_by_filter(
                generator.filter.as_ref(),
                key,
                object.size().unwrap_or_default(),
                object
                    .last_modified()
                    .map(::aws_sdk_s3::primitives::DateTime::secs),
                object
                    .storage_class()
                    .map(::aws_sdk_s3::types::ObjectStorageClass::as_str),
            ) {
                continue;
            }
            if entries.len() >= MAX_JOB_TASKS {
                return Err(format!(
                    "This emulator runs at most {MAX_JOB_TASKS} tasks in one job; the generated \
                     manifest names more."
                ));
            }
            entries.push(ManifestEntry {
                bucket: source.clone(),
                key: key.to_string(),
                version_id: None,
            });
        }
        continuation = page.next_continuation_token().map(str::to_string);
        pages += 1;
        if continuation.is_none() || pages >= MAX_LISTING_PAGES {
            break;
        }
    }

    // "Determines whether the generated manifest should be saved to Amazon S3"
    // (`S3JobManifestGenerator$EnableManifestOutput`): only a generator asked
    // for it writes one, and only where the request said to put it.
    let generated = match (
        generator.enable_manifest_output,
        generator.manifest_output_location.as_ref(),
    ) {
        (true, Some(location)) => {
            let Some(bucket) = arn_bucket(&location.bucket) else {
                return Err(format!(
                    "The generator's ManifestOutputLocation names no bucket: {}",
                    location.bucket
                ));
            };
            let prefix = job_prefix(location.manifest_prefix.as_deref(), job_id);
            let data_key = format!("{prefix}/{GENERATED_MANIFEST_NAME}");
            let data = generated_manifest_csv(&entries);
            let data_size = data.len();
            let data_written = client
                .put_object()
                .bucket(&bucket)
                .key(&data_key)
                .body(::aws_sdk_s3::primitives::ByteStream::from(data))
                .send()
                .await
                .map_err(|error| {
                    format!(
                        "The generated manifest could not be written: {}",
                        refused_by_s3(&error).message
                    )
                })?;
            // A manifest in this format is READ from its own `manifest.json`,
            // which names the data file beside it — so the job writes that
            // object too and locates the manifest by it.
            let manifest_key = format!("{prefix}/{GENERATED_MANIFEST_MANIFEST_NAME}");
            let manifest_written = client
                .put_object()
                .bucket(&bucket)
                .key(&manifest_key)
                .body(::aws_sdk_s3::primitives::ByteStream::from(
                    inventory_manifest_json(
                        &source,
                        &bucket,
                        &data_key,
                        data_size,
                        data_written
                            .e_tag()
                            .map(|e_tag| e_tag.trim_matches('"')),
                        ::aws_smithy_types::DateTime::from(SystemTime::now()),
                    ),
                ))
                .send()
                .await
                .map_err(|error| {
                    format!(
                        "The generated manifest could not be written: {}",
                        refused_by_s3(&error).message
                    )
                })?;
            Some(S3GeneratedManifestDescriptor {
                format: Some(generated_manifest_format()),
                location: Some(written_manifest_location(
                    &bucket,
                    &manifest_key,
                    manifest_written.e_tag(),
                )),
            })
        }
        _ => None,
    };
    Ok(Listed { entries, generated })
}

/// Carry out one task — the job's action against one object of its list —
/// through Amazon S3's own API, and report what that answered.
async fn run_task(
    client: &::aws_sdk_s3::Client,
    action: &Action,
    entry: &ManifestEntry,
) -> TaskOutcome {
    let refusal = match action {
        Action::Copy(copy) => copy_object(client, copy, entry).await.err(),
        Action::PutTagging(tag_set) => match tagging(tag_set) {
            Some(tagging) => client
                .put_object_tagging()
                .bucket(&entry.bucket)
                .key(&entry.key)
                .set_version_id(entry.version_id.clone())
                .tagging(tagging)
                .send()
                .await
                .err()
                .map(|error| refused_by_s3(&error)),
            None => Some(Refusal {
                code: REQUEST_INVALID_CODE.to_string(),
                status: Some(REQUEST_INVALID_STATUS),
                message: "The job's tag set is not a tagging document Amazon S3 takes."
                    .to_string(),
            }),
        },
        Action::DeleteTagging => client
            .delete_object_tagging()
            .bucket(&entry.bucket)
            .key(&entry.key)
            .set_version_id(entry.version_id.clone())
            .send()
            .await
            .err()
            .map(|error| refused_by_s3(&error)),
    };
    match refusal {
        None => TaskOutcome {
            entry: entry.clone(),
            status: TASK_SUCCEEDED,
            error_code: String::new(),
            http_status: Some(TASK_SERVED_STATUS),
            message: TASK_SUCCEEDED_MESSAGE.to_string(),
        },
        Some(refusal) => TaskOutcome {
            entry: entry.clone(),
            status: TASK_FAILED,
            error_code: refusal.code,
            http_status: refusal.status,
            message: refusal.message,
        },
    }
}

/// The HTTP status a task Amazon S3 carried out was answered with.
const TASK_SERVED_STATUS: u16 = 200;

/// Copy one object of the list into the job's target bucket, under the target
/// key prefix: "S3 Batch Operations passes every object to the underlying
/// `CopyObject` API operation", so the copy is that operation's own, with the
/// parameters the job's `S3CopyObjectOperation` carries.
async fn copy_object(
    client: &::aws_sdk_s3::Client,
    copy: &S3CopyObjectOperation,
    entry: &ManifestEntry,
) -> Result<(), Refusal> {
    let Some(target) = copy.target_resource.as_deref().and_then(arn_bucket) else {
        return Err(Refusal {
            code: REQUEST_INVALID_CODE.to_string(),
            status: Some(REQUEST_INVALID_STATUS),
            message: format!(
                "The copy's TargetResource names no bucket: {}",
                copy.target_resource.as_deref().unwrap_or_default()
            ),
        });
    };
    client
        .copy_object()
        .bucket(target)
        .key(prefixed_key(copy.target_key_prefix.as_deref(), &entry.key))
        .copy_source(copy_source(entry))
        .set_storage_class(
            copy.storage_class
                .as_deref()
                .map(::aws_sdk_s3::types::StorageClass::from),
        )
        .set_metadata_directive(
            copy.metadata_directive
                .as_deref()
                .map(::aws_sdk_s3::types::MetadataDirective::from),
        )
        .send()
        .await
        .map_err(|error| refused_by_s3(&error))?;
    Ok(())
}

/// The `x-amz-copy-source` of one manifest entry: "the name of the source
/// bucket and the key of the source object, separated by a slash (/) … The
/// value must be URL-encoded", with `?versionId=` naming a stored version.
///
/// The key is percent-encoded with the `percent-encoding` crate over the set
/// RFC 3986 reserves, so a key carrying a space, a `?` or a `#` names the object
/// it is rather than a different one.
fn copy_source(entry: &ManifestEntry) -> String {
    let encoded = |value: &str| {
        ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC)
            .to_string()
    };
    let source = format!("{}/{}", entry.bucket, encoded(&entry.key));
    match entry.version_id.as_deref() {
        Some(version_id) => format!("{source}?versionId={}", encoded(version_id)),
        None => source,
    }
}

/// The job's tag set as Amazon S3's own `Tagging` document, or `None` for a tag
/// set that document cannot carry.
fn tagging(tag_set: &[S3Tag]) -> Option<::aws_sdk_s3::types::Tagging> {
    let tags: Vec<::aws_sdk_s3::types::Tag> = tag_set
        .iter()
        .filter_map(|tag| {
            // Both members of a tag are present on the stored tag set, so there
            // is nothing here for the builder to reject.
            ::aws_sdk_s3::types::Tag::builder()
                .key(&tag.key)
                .value(&tag.value)
                .build()
                .ok()
        })
        .collect();
    ::aws_sdk_s3::types::Tagging::builder()
        .set_tag_set(Some(tags))
        .build()
        .ok()
}

/// Write the job's completion report: the result file accounting for the tasks
/// the report's scope asks for, and the report's own manifest naming it.
///
/// Both go under `<Prefix>/job-<JobId>/` in the report's bucket, which is where
/// `JobReport$Prefix` puts "the job-completion report" of one job.
async fn write_completion_report(
    client: &::aws_sdk_s3::Client,
    report: &JobReport,
    job_id: &str,
    outcomes: &[TaskOutcome],
) -> Result<(), String> {
    let Some(bucket) = report.bucket.as_deref().and_then(arn_bucket) else {
        return Err(format!(
            "The report's Bucket names no bucket: {}",
            report.bucket.as_deref().unwrap_or_default()
        ));
    };
    let prefix = job_prefix(report.prefix.as_deref(), job_id);
    let results_key = format!("{prefix}/{REPORT_RESULTS_NAME}");
    let written = client
        .put_object()
        .bucket(&bucket)
        .key(&results_key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            report_results_csv(outcomes, report.report_scope.as_deref()),
        ))
        .send()
        .await
        .map_err(|error| {
            format!(
                "The job-completion report could not be written: {}",
                refused_by_s3(&error).message
            )
        })?;
    // The report's `MD5Checksum` is the digest of the result file, which is
    // what Amazon S3 computed for the object it just stored: the `ETag` of that
    // write, unquoted.
    let md5 = written
        .e_tag()
        .map(|e_tag| e_tag.trim_matches('"').to_string());
    client
        .put_object()
        .bucket(&bucket)
        .key(format!("{prefix}/{REPORT_MANIFEST_NAME}"))
        .body(::aws_sdk_s3::primitives::ByteStream::from(
            report_manifest_json(
                &bucket,
                &results_key,
                md5.as_deref(),
                ::aws_smithy_types::DateTime::from(SystemTime::now()),
                outcomes.iter().all(TaskOutcome::succeeded),
            ),
        ))
        .send()
        .await
        .map_err(|error| {
            format!(
                "The job-completion report could not be written: {}",
                refused_by_s3(&error).message
            )
        })?;
    Ok(())
}

/// How Amazon S3 refused one request, as the completion report accounts for it.
#[derive(Debug, Clone)]
struct Refusal {
    /// The error code the answer carried.
    code: String,
    /// The HTTP status it carried, or `None` when the request reached no answer
    /// at all — a status nothing answered with is not reported as one.
    status: Option<u16>,
    /// The message it carried.
    message: String,
}

/// What an Amazon S3 answer refused a request with: its own code, status and
/// message, read off the answer rather than restated here — so a task's row in
/// the completion report says what the object store said.
fn refused_by_s3<E: ProvideErrorMetadata>(
    error: &SdkError<E, ::aws_smithy_runtime_api::http::Response>,
) -> Refusal {
    Refusal {
        code: error.code().unwrap_or(UNKNOWN_REFUSAL_CODE).to_string(),
        status: error
            .raw_response()
            .map(|answer| answer.status().as_u16()),
        message: error
            .message()
            .unwrap_or(UNKNOWN_REFUSAL_MESSAGE)
            .to_string(),
    }
}

/// The code a task's row carries when the answer that refused it named none —
/// an answer that reached no handler at all.
const UNKNOWN_REFUSAL_CODE: &str = "InternalError";

/// See [`UNKNOWN_REFUSAL_CODE`].
const UNKNOWN_REFUSAL_MESSAGE: &str = "The task's request was not answered by Amazon S3.";

/// An Amazon S3 client over the in-process callback into this same emulator.
fn s3_client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_s3::Client {
    let region = ::service_common::arn::region_label(region).unwrap_or(DEFAULT_REGION);
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket
        // travels in the path rather than in a per-bucket host the router could
        // not resolve.
        .force_path_style(true)
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching
        // the identity — the credentials the callback hands over are used
        // directly, and they only sign these in-process calls.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}
