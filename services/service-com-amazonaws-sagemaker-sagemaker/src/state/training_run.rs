// SPDX-License-Identifier: CC0-1.0

//! The SERVER-SIDE RUN of an accepted training job: the training data it reads out of the
//! channels the request named, the model it fits on them, the model artifact it writes
//! under the job's output location, and the metrics it measured.
//!
//! # What a training job DOES
//!
//! "After training completes, SageMaker saves the resulting model artifacts to an Amazon S3
//! location that you specify" (`CreateTrainingJob`'s own doc), and `DescribeTrainingJob`
//! reports that object as `ModelArtifacts$S3ModelArtifacts` — "the path of the S3 object
//! that contains the model artifacts". So an accepted job's work is: read the channels,
//! produce a model, write it where the request asked, and report the object. Nothing in the
//! API says HOW the work is performed.
//!
//! This module performs it IN PROCESS, over the caller's own data:
//!
//! * every channel's `S3DataSource` is read out of this emulator's own Amazon S3 over the
//!   in-process callback, bounded by [`MAX_OBJECT_BYTES`] per object,
//!   [`MAX_CHANNEL_OBJECTS`] objects per channel and [`MAX_RUN_INPUT_BYTES`] over the run;
//! * the records are read as comma-separated values — the form a training channel carries
//!   for SageMaker's built-in algorithms, which document "the target variable in the first
//!   column and the input data in the remaining columns" and no header record — by the
//!   maintained CSV reader rather than by splitting text here;
//! * a least-squares LINEAR model is fitted on them by batch gradient descent, bounded by
//!   [`MAX_FIT_OPERATIONS`]: every number the run reports is measured from the caller's
//!   data;
//! * the fitted model is serialized and written as the gzip-compressed tar `model.tar.gz`
//!   ("a SageMaker container stores your trained model artifacts in the /opt/ml/model
//!   directory. After training has completed, by default, these artifacts are uploaded to
//!   your Amazon S3 bucket as compressed files" — `ModelArtifacts`) under
//!   `<S3OutputPath>/<TrainingJobName>/output/`, so the object the describe names is an
//!   object Amazon S3 itself holds;
//! * the root-mean-square error of the fitted model is measured over each channel and
//!   reported as the run's final metrics.
//!
//! The metric a run reports is therefore the one THIS fit measured, named for the channel it
//! was measured over (`<channel>:rmse`). Nothing here runs the algorithm image the
//! `AlgorithmSpecification` names, so the run emits neither a built-in algorithm's own metric
//! names nor the `MetricDefinitions` a custom image's log lines would be scraped for, and the
//! `HyperParameters` the request carried are reported as the job's own (and recorded in the
//! artifact) rather than steering the fit. A caller that branches on a particular metric NAME
//! will not find it; one that reads `FinalMetricDataList` finds a value that was genuinely
//! measured over the data it supplied.
//!
//! The run is the JOB's own work, so what it could not do is the job's modelled `Failed`
//! status with the `FailureReason` naming the condition that failed — never a refusal of the
//! `CreateTrainingJob` request that submitted it, and never a `Completed` status for a run
//! that wrote no artifact.
//!
//! What fails a run is a LEG of its work that failed: an object it could not download under
//! the execution role, or an artifact it could not write. The SHAPE of the caller's data is
//! not one of them — nothing in the API models what a channel's bytes mean, `Channel$ContentType`
//! being the caller's own MIME type and the algorithm, not SageMaker, reading them — so data
//! that holds no record of the form above leaves a run that still fits, writes its artifact
//! and completes, with how many rows held no record reported in that artifact.
//!
//! The stages of the work are reported as they happen: the run records the `SecondaryStatus`
//! phases it got through ([`TrainingRunOutcome::phases`]) and the clock reveals them in order
//! ([`super::TrainingJob::settle_status`]), so a caller polling the job sees the data come
//! down, the fit run and the artifact go up before the terminal status.
//!
//! # The job runs AS the execution role
//!
//! "RoleArn - The Amazon Resource Name (ARN) that SageMaker assumes to perform tasks on your
//! behalf during model training. You must grant this role the necessary permissions so that
//! SageMaker can successfully complete model training" (`CreateTrainingJob`), and
//! `S3DataSource$S3Uri`'s own doc: "the object that each S3Uri points to must be readable by
//! the IAM role that SageMaker uses to perform tasks on your behalf". So each object the run
//! reads and the artifact it writes is a call made UNDER that role: the decision is IAM's,
//! asked of IAM through the shared reading
//! ([`::service_common::iam_role::role_withholds_action`]), and an action IAM withholds from
//! the role is one the run does not perform — the job fails with the reason naming that
//! condition. Where IAM answers nothing (no IAM mounted, a role this emulator holds no
//! record of) nothing is withheld, for the reason that module gives.
//!
//! The create asks the same question of the INPUT DATA SOURCES before it accepts the job at
//! all ([`absent_input_objects`]): a location that answers no object the role may list backs
//! no run, so it is refused synchronously instead of settling a job that could only fail.

use super::*;

use crate::ops::types::{CreateTrainingJobInput, MetricData};

/// The most bytes the run reads out of ONE object.
///
/// An EMULATOR ceiling, not an AWS limit: the run holds what it reads in memory, so the read
/// is bounded in every direction the caller's data can grow.
pub const MAX_OBJECT_BYTES: usize = 1_048_576;

/// The most objects the run reads out of ONE channel's location.
pub const MAX_CHANNEL_OBJECTS: usize = 16;

/// The most bytes the run reads across every channel of one job.
pub const MAX_RUN_INPUT_BYTES: usize = 4_194_304;

/// The most records the fit is given, out of those the channels hold.
pub const MAX_RECORDS: usize = 5_000;

/// The most feature columns one record contributes; a record wider than this is fitted on
/// its first [`MAX_FEATURES`] features, which is what the artifact then reports.
pub const MAX_FEATURES: usize = 32;

/// The work one fit may do, counted in (record, feature) updates: it is what decides how
/// many epochs the descent runs ([`fit_epochs`]), so the fit is bounded whatever the data's
/// shape.
pub const MAX_FIT_OPERATIONS: usize = 2_000_000;

/// The most epochs the descent runs, however small the data is.
pub const MAX_EPOCHS: usize = 200;

/// The step the descent takes along the gradient of the mean squared error. Fixed, so one
/// dataset fits to one model.
const LEARNING_RATE: f64 = 0.5;

/// The channel SageMaker's built-in algorithms take their training data from ("specify the
/// training data in the `train` channel"). The run fits on this channel's records when the
/// job declares one.
pub const TRAIN_CHANNEL: &str = "train";

/// The entry the model archive holds: the fitted model, as this run serialized it.
pub const MODEL_ARCHIVE_ENTRY: &str = "model.json";

/// The object a finished run writes under `<S3OutputPath>/<TrainingJobName>/output/`
/// (`ModelArtifacts$S3ModelArtifacts`: "for example,
/// `s3://bucket-name/keynameprefix/model.tar.gz`").
pub const MODEL_ARCHIVE_OBJECT: &str = "model.tar.gz";

/// The `S3DataType` whose `S3Uri` "identifies an object that is a manifest file containing a
/// list of object keys that you want SageMaker to use for model training"
/// (`S3DataSource$S3DataType`). Every other value identifies a key name prefix, which is how
/// the run reads it.
pub const MANIFEST_FILE: &str = "ManifestFile";

// The IAM actions an Amazon S3 read or write is authorized under, the partition an Amazon S3
// ARN carries and the location an `s3://` URI names are this service's ONE reading of an
// Amazon S3 location ([`super::s3_access`]), reached here through `use super::*`.

/// What a training job's run produced, recorded when the run reported and revealed by the
/// clock ([`super::TrainingJob::settle_status`]).
#[derive(Debug, Clone)]
pub enum TrainingRunOutcome {
    /// The run finished: the model artifact object it wrote, and the metrics it measured.
    Completed {
        /// The URI of the object Amazon S3 now holds, which is what
        /// `ModelArtifacts$S3ModelArtifacts` reports.
        model_artifacts: String,
        /// "The name, value, and date and time of a metric that was emitted"
        /// (`MetricData`), one per channel the run measured the fitted model over.
        final_metrics: Vec<MetricData>,
        /// The stages of its work the run GOT THROUGH, in the order it did (see
        /// [`TrainingRunOutcome::phases`]).
        phases: Vec<String>,
    },
    /// The run did not finish, and this is the condition that stopped it — the job's
    /// `FailureReason`.
    Failed {
        reason: String,
        /// The stages the run had got through when the condition stopped it, so the job's
        /// history says WHERE its work failed (see [`TrainingRunOutcome::phases`]).
        phases: Vec<String>,
    },
}

impl TrainingRunOutcome {
    /// A run stopped by `reason` before it got through any stage of its work.
    #[must_use]
    pub fn failed(reason: impl Into<String>) -> Self {
        Self::failed_in(Vec::new(), reason)
    }

    /// A run stopped by `reason` once it had got through `phases`, bounded to the modelled
    /// `FailureReason` length (0..1024).
    #[must_use]
    pub fn failed_in(phases: Vec<String>, reason: impl Into<String>) -> Self {
        let reason: String = reason.into();
        let bounded = match reason.char_indices().nth(1024) {
            Some((at, _)) => reason[..at].to_string(),
            None => reason,
        };
        Self::Failed {
            reason: bounded,
            phases,
        }
    }

    /// The `SecondaryStatus` stages of its own work this run went through, after the
    /// `Starting` the job is accepted in: the data it downloaded to the ML storage volumes,
    /// the fit it performed and the artifact it uploaded, each named by the member
    /// `DescribeTrainingJob$SecondaryStatus`'s doc gives it.
    ///
    /// They are what the job ENTERED, so a run that stopped on one of its legs reports the
    /// stages before that leg and not the ones it never reached.
    #[must_use]
    pub fn phases(&self) -> &[String] {
        match self {
            Self::Completed { phases, .. } | Self::Failed { phases, .. } => phases,
        }
    }
}

/// How SageMaker words a training job that could not read a channel's data: the
/// `ClientError:` class a failure of the caller's own input carries, then the leg that
/// failed (the data download) and what failed on it.
#[must_use]
pub fn data_download_failed(channel: &str, uri: &str, detail: &str) -> String {
    format!("ClientError: Data download failed:{detail} for channel {channel} at {uri}")
}

/// What [`data_download_failed`] carries for a read the execution role is not allowed to
/// make: the Amazon S3 answer such a read gets, named by its own error code and status.
#[must_use]
pub fn access_denied_detail() -> String {
    "AccessDenied (Service: Amazon S3; Status Code: 403; Error Code: AccessDenied)".to_string()
}

/// What [`data_download_failed`] carries for a location holding no object at all.
const NO_OBJECT_DETAIL: &str = "no object was found at the S3 location";

/// What [`data_download_failed`] carries for a read Amazon S3 refused for anything else: the
/// error code it answered with, and otherwise that nothing could be read — never a code
/// Amazon S3 did not answer.
#[must_use]
fn s3_answer_detail(code: Option<&str>, unanswered: &str) -> String {
    match code {
        Some(code) => format!("{code} (Service: Amazon S3)"),
        None => unanswered.to_string(),
    }
}

/// [`s3_answer_detail`]'s wording for a read Amazon S3 answered nothing readable to.
const UNREAD_OBJECT_DETAIL: &str = "the object could not be read from Amazon S3";

/// [`s3_answer_detail`]'s wording for a write Amazon S3 answered nothing readable to.
const UNWRITTEN_OBJECT_DETAIL: &str = "the object could not be written to Amazon S3";

/// How SageMaker words a run whose model artifacts could not be written: the upload leg that
/// failed and what failed on it.
#[must_use]
pub fn model_upload_failed(uri: &str, detail: &str) -> String {
    format!("ClientError: Model artifact upload failed:{detail} for {uri}")
}

/// One record of a training channel: the target variable the first column carries, and the
/// features the remaining columns carry.
#[derive(Debug, Clone)]
pub struct Record {
    pub target: f64,
    pub features: Vec<f64>,
}

/// What one channel contributed to the run.
#[derive(Debug, Clone)]
pub struct ChannelData {
    /// The channel's own name, which the metric it is measured over is named after.
    pub name: String,
    /// The location its records were read from, which a failure about it names.
    pub uri_read: String,
    /// How many objects of its location were read.
    pub objects: usize,
    /// The records read from them.
    pub records: Vec<Record>,
    /// The rows that held no record of the channel's form, which the artifact reports.
    pub skipped: usize,
}

impl ChannelData {
    /// One channel's read, before anything has been read into it.
    pub fn new(name: &str, uri_read: String) -> Self {
        Self {
            name: name.to_string(),
            uri_read,
            objects: 0,
            records: Vec::new(),
            skipped: 0,
        }
    }
}

/// A fitted linear model: `prediction = intercept + weights · features`, in the caller's own
/// scale.
#[derive(Debug, Clone)]
pub struct LinearModel {
    pub intercept: f64,
    pub weights: Vec<f64>,
    /// How many passes over the data the descent took, which the artifact reports.
    pub epochs: usize,
}

impl LinearModel {
    /// What this model predicts for one record's features.
    pub fn predict(&self, features: &[f64]) -> f64 {
        self.intercept
            + self
                .weights
                .iter()
                .zip(features)
                .map(|(weight, value)| weight * value)
                .sum::<f64>()
    }

    /// The root-mean-square error of this model over `records` — a MEASUREMENT over real
    /// predictions, which is what the run reports as its final metric.
    pub fn rmse(&self, records: &[Record]) -> f64 {
        if records.is_empty() {
            return 0.0;
        }
        let squared: f64 = records
            .iter()
            .map(|record| (self.predict(&record.features) - record.target).powi(2))
            .sum();
        (squared / records.len() as f64).sqrt()
    }
}

// The location an `s3://` URI names — `S3Location`, with its `uri`, `object_arn` and
// `bucket_arn` readings — lives in [`super::s3_access`], the one place this service reads an
// Amazon S3 location, and is reached here through `use super::*`.

/// How many epochs a dataset of `records` rows and `columns` features is fitted for: as many
/// as [`MAX_FIT_OPERATIONS`] admits, never more than [`MAX_EPOCHS`] and never none.
#[must_use]
pub fn fit_epochs(records: usize, columns: usize) -> usize {
    let per_epoch = records.saturating_mul(columns.max(1)).max(1);
    (MAX_FIT_OPERATIONS / per_epoch).clamp(1, MAX_EPOCHS)
}

/// Why SageMaker will not accept this training job: an input data source whose Amazon S3
/// location names no object the execution role can list — and `None` when every channel's
/// location answers at least one, which is also the answer wherever nothing was LEARNED.
///
/// The input data sources are settled SYNCHRONOUSLY, before the job is created: "the object
/// that each S3Uri points to must be readable by the IAM role that SageMaker uses to perform
/// tasks on your behalf" (`S3DataSource$S3Uri`), so a location that answers no object backs
/// no run of this job and the create is refused for it rather than accepted into a job that
/// could only fail. The listing is made with the ROLE's grant on the bucket decided by IAM
/// ([`::service_common::iam_role::role_withholds_action`]), which is the permission the
/// refusal names.
///
/// Only what was ANSWERED refuses: a listing that could not be made at all — no Amazon S3 to
/// ask — teaches nothing about the location, so it refuses nothing and the run reports what
/// it finds. A role nothing is known about is refused nothing, for the reason the shared
/// reading gives.
///
/// The RUN authorizes again, per OBJECT, for the objects only it can know: a prefix resolves
/// to concrete keys when it is read, which this admission cannot name in advance.
pub async fn absent_input_objects(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    request: &CreateTrainingJobInput,
) -> Option<String> {
    let partition = arn_partition(&request.role_arn);
    let client = s3_client(ctx, region);
    for channel in request.input_data_config.iter().flatten() {
        let Some(source) = channel.data_source.s3_data_source.as_ref() else {
            continue;
        };
        let Some(location) = S3Location::parse(&source.s3_uri) else {
            continue;
        };
        let refusal = || {
            crate::state::no_s3_objects_found(
                &source.s3_uri,
                region,
                &request.role_arn,
                &location.bucket,
            )
        };
        if ::service_common::iam_role::role_withholds_action(
            ctx,
            region,
            &request.role_arn,
            LIST_BUCKET_ACTION,
            &location.bucket_arn(&partition),
        )
        .await
        {
            return Some(refusal());
        }
        let listed = client
            .list_objects_v2()
            .bucket(&location.bucket)
            .prefix(&location.key)
            .max_keys(1)
            .send()
            .await;
        // A listing that ANSWERED, with no object under the prefix, is the refusal; one that
        // could not be made says nothing (see the doc above).
        if listed.is_ok_and(|listed| listed.contents().is_empty()) {
            return Some(refusal());
        }
    }
    None
}

/// Run one accepted training job: read its channels, fit a model on them, write the model
/// artifact under the output location the request named, and report what was produced.
///
/// Answers a [`TrainingRunOutcome`] rather than an error channel, for the reason the module
/// docs give: the run is the job's own work.
pub async fn run_training_job(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    job_name: &str,
    request: &CreateTrainingJobInput,
) -> TrainingRunOutcome {
    let Some(output) = S3Location::parse(&request.output_data_config.s3_output_path) else {
        return TrainingRunOutcome::failed(model_upload_failed(
            &request.output_data_config.s3_output_path,
            "the output location names no Amazon S3 bucket",
        ));
    };
    let partition = arn_partition(&request.role_arn);
    let client = s3_client(ctx, region);

    // The stages of its own work this run gets through, recorded as it enters each one so a
    // run that stops on a leg reports the ones before it (see [`TrainingRunOutcome::phases`]).
    let mut phases: Vec<String> = Vec::new();
    // "Downloading - An optional stage for algorithms that support File training input mode.
    // It indicates that data is being downloaded to the ML storage volumes"
    // (`DescribeTrainingJob$SecondaryStatus`): the stage belongs to a run that HAS data to
    // bring down, so a job whose channels name no Amazon S3 location never enters it.
    if request
        .input_data_config
        .iter()
        .flatten()
        .any(|channel| channel.data_source.s3_data_source.is_some())
    {
        phases.push(TRAINING_SECONDARY_STATUS_DOWNLOADING.to_string());
    }

    let mut remaining_bytes = MAX_RUN_INPUT_BYTES;
    let mut channels: Vec<ChannelData> = Vec::new();
    for channel in request.input_data_config.iter().flatten() {
        let Some(source) = channel.data_source.s3_data_source.as_ref() else {
            // A channel whose data lives in a file system rather than in Amazon S3 takes no
            // part in the fit; a job whose channels hold no record at all fails below.
            continue;
        };
        match read_channel(
            ctx,
            &client,
            region,
            &request.role_arn,
            &partition,
            channel.channel_name.as_str(),
            source.s3_data_type.as_str(),
            source.s3_uri.as_str(),
            &mut remaining_bytes,
        )
        .await
        {
            Ok(read) => channels.push(read),
            Err(reason) => return TrainingRunOutcome::failed_in(phases, reason),
        }
    }

    // The records the model is FITTED on: the training channel's when the job declares one,
    // and otherwise every record the job's channels hold, narrowed to the ONE shape a model
    // can be fitted on — the feature count of the first record — so a channel of another
    // width takes no part rather than being read against the wrong columns.
    //
    // Data that holds no record of that form is not a failure of the JOB: nothing in the API
    // models what a channel's bytes mean — `Channel$ContentType` is the caller's own MIME
    // type and it is the algorithm, not SageMaker, that reads them — so a run whose download
    // and upload legs both succeeded completes, fitted on whatever records it could read and
    // reporting a metric only for the channels it measured one over. How many rows held no
    // record is reported in the artifact the run writes.
    let mut fitted_on: Vec<Record> = match channels.iter().find(|read| read.name == TRAIN_CHANNEL)
    {
        Some(train) => train.records.clone(),
        None => channels
            .iter()
            .flat_map(|read| read.records.iter().cloned())
            .collect(),
    };
    let columns = fitted_on.first().map_or(0, |record| record.features.len());
    fitted_on.retain(|record| record.features.len() == columns);

    // "Training - Training is in progress" (`DescribeTrainingJob$SecondaryStatus`): the fit
    // the run performs over the records it read.
    phases.push(TRAINING_SECONDARY_STATUS_TRAINING.to_string());

    // Fitting a model and measuring it are synchronous, CPU-bound work with no await of
    // their own, so they go to the blocking pool and the worker serving this request is
    // released while they run. The work itself is bounded ([`MAX_FIT_OPERATIONS`]), so the
    // pool is never occupied for long.
    let measured_at = timestamp(SystemTime::now());
    let measured: Vec<(String, Vec<Record>)> = channels
        .iter()
        .filter(|read| !read.records.is_empty())
        .map(|read| (read.name.clone(), read.records.clone()))
        .collect();
    let fit_and_measure = ::tokio::task::spawn_blocking(move || {
        let model = fit(&fitted_on, fit_epochs(fitted_on.len(), columns));
        let metrics: Vec<MetricData> = measured
            .iter()
            .map(|(name, records)| MetricData {
                // The metric name SageMaker's built-in regression algorithms emit for the
                // error they measure over a channel: the channel's own name, then the
                // metric.
                metric_name: Some(format!("{name}:rmse")),
                value: Some(model.rmse(records) as f32),
                timestamp: Some(measured_at),
            })
            .collect();
        (model, metrics)
    })
    .await;
    let Ok((model, final_metrics)) = fit_and_measure else {
        return TrainingRunOutcome::failed_in(phases, crate::state::INTERNAL_FAILURE_REASON);
    };

    // "Uploading - Training is complete and the model artifacts are being uploaded to the S3
    // location" (`DescribeTrainingJob$SecondaryStatus`).
    phases.push(TRAINING_SECONDARY_STATUS_UPLOADING.to_string());
    let artifact = artifact_location(&output, job_name);
    let document = model_document(job_name, request, &model, &channels, &final_metrics);
    let Ok(archive) = model_archive(&document) else {
        return TrainingRunOutcome::failed_in(
            phases,
            model_upload_failed(&artifact.uri(), "the model archive could not be built"),
        );
    };
    if ::service_common::iam_role::role_withholds_action(
        ctx,
        region,
        &request.role_arn,
        PUT_OBJECT_ACTION,
        &artifact.object_arn(&partition),
    )
    .await
    {
        return TrainingRunOutcome::failed_in(
            phases,
            model_upload_failed(&artifact.uri(), &access_denied_detail()),
        );
    }
    if let Err(detail) = write_object(&client, &artifact, archive).await {
        return TrainingRunOutcome::failed_in(
            phases,
            model_upload_failed(&artifact.uri(), &detail),
        );
    }
    TrainingRunOutcome::Completed {
        model_artifacts: artifact.uri(),
        final_metrics,
        phases,
    }
}

/// Where a finished run writes the model artifact: under the output location the request
/// named, in the job's own folder — the same path [`super::TrainingJob::model_artifacts_path`]
/// reports while the job is still running.
fn artifact_location(output: &S3Location, job_name: &str) -> S3Location {
    let prefix = output.key.trim_end_matches('/');
    let key = if prefix.is_empty() {
        format!("{job_name}/output/{MODEL_ARCHIVE_OBJECT}")
    } else {
        format!("{prefix}/{job_name}/output/{MODEL_ARCHIVE_OBJECT}")
    };
    S3Location {
        bucket: output.bucket.clone(),
        key,
    }
}

/// Read one channel's records out of its Amazon S3 location.
///
/// `Err` is the job's `FailureReason`: the download leg failed, which is the job's own
/// failure rather than the request's.
#[allow(clippy::too_many_arguments)]
pub async fn read_channel(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    client: &::aws_sdk_s3::Client,
    region: &str,
    role_arn: &str,
    partition: &str,
    name: &str,
    s3_data_type: &str,
    s3_uri: &str,
    remaining_bytes: &mut usize,
) -> Result<ChannelData, String> {
    let Some(location) = S3Location::parse(s3_uri) else {
        return Err(data_download_failed(
            name,
            s3_uri,
            "the channel names no Amazon S3 bucket",
        ));
    };
    let mut read = ChannelData::new(name, location.uri());
    let objects = channel_objects(
        ctx,
        client,
        region,
        role_arn,
        partition,
        name,
        s3_data_type,
        &location,
    )
    .await?;
    if objects.is_empty() {
        return Err(data_download_failed(name, &location.uri(), NO_OBJECT_DETAIL));
    }
    for object in objects {
        if ::service_common::iam_role::role_withholds_action(
            ctx,
            region,
            role_arn,
            GET_OBJECT_ACTION,
            &object.object_arn(partition),
        )
        .await
        {
            return Err(data_download_failed(
                name,
                &object.uri(),
                &access_denied_detail(),
            ));
        }
        let ceiling = MAX_OBJECT_BYTES.min(*remaining_bytes);
        if ceiling == 0 {
            break;
        }
        let bytes = read_object(client, &object, ceiling)
            .await
            .map_err(|detail| data_download_failed(name, &object.uri(), &detail))?;
        *remaining_bytes = remaining_bytes.saturating_sub(bytes.len());
        read.objects += 1;
        read_records(&bytes, &mut read);
        if read.records.len() >= MAX_RECORDS {
            break;
        }
    }
    Ok(read)
}

/// The objects one channel's data lives in.
///
/// A location whose `S3DataType` is [`MANIFEST_FILE`] names a manifest, whose first element
/// is a prefix and whose remaining elements are suffixes of it (`S3DataSource$S3Uri`) — the
/// prefix element is an `s3://` URI, so the objects it names are objects of ITS bucket, which
/// is carried alongside each key rather than assumed to be the manifest's own. Every other
/// value names a key name prefix, so the objects are the ones Amazon S3 lists under it — which
/// is the object itself when the prefix names one.
#[allow(clippy::too_many_arguments)]
async fn channel_objects(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    client: &::aws_sdk_s3::Client,
    region: &str,
    role_arn: &str,
    partition: &str,
    channel: &str,
    s3_data_type: &str,
    location: &S3Location,
) -> Result<Vec<S3Location>, String> {
    if s3_data_type == MANIFEST_FILE {
        if ::service_common::iam_role::role_withholds_action(
            ctx,
            region,
            role_arn,
            GET_OBJECT_ACTION,
            &location.object_arn(partition),
        )
        .await
        {
            return Err(data_download_failed(
                channel,
                &location.uri(),
                &access_denied_detail(),
            ));
        }
        let bytes = read_object(client, location, MAX_OBJECT_BYTES)
            .await
            .map_err(|detail| data_download_failed(channel, &location.uri(), &detail))?;
        return Ok(manifest_objects(&bytes, location));
    }
    let listed = client
        .list_objects_v2()
        .bucket(&location.bucket)
        .prefix(&location.key)
        .max_keys(i32::try_from(MAX_CHANNEL_OBJECTS).unwrap_or(i32::MAX))
        .send()
        .await;
    let Ok(listed) = listed else {
        // The listing is not a grant the run depends on: a location the role may read
        // object by object is read that way, so a listing that answered nothing leaves the
        // object the channel named as the one to read.
        return Ok(vec![location.clone()]);
    };
    let objects: Vec<S3Location> = listed
        .contents()
        .iter()
        .filter_map(|object| object.key().map(str::to_string))
        // A key ending in the delimiter is a folder marker rather than training data.
        .filter(|key| !key.ends_with('/'))
        .take(MAX_CHANNEL_OBJECTS)
        .map(|key| S3Location {
            bucket: location.bucket.clone(),
            key,
        })
        .collect();
    Ok(objects)
}

/// The objects a manifest names: "the first element is a prefix which is followed by one or
/// more suffixes… SageMaker appends the suffix elements to the prefix"
/// (`S3DataSource$S3Uri`).
///
/// The prefix element is itself an `s3://` URI, so each object answered here is an object of
/// THAT URI's bucket; a manifest that names no prefix leaves the objects in the manifest's own
/// bucket (`held_in`), which is where a suffix with nothing in front of it resolves.
fn manifest_objects(bytes: &[u8], held_in: &S3Location) -> Vec<S3Location> {
    let Ok(::serde_json::Value::Array(elements)) = ::serde_json::from_slice(bytes) else {
        return Vec::new();
    };
    let mut prefix = S3Location {
        bucket: held_in.bucket.clone(),
        key: String::new(),
    };
    let mut objects = Vec::new();
    for element in elements.iter().take(MAX_CHANNEL_OBJECTS + 1) {
        match element {
            ::serde_json::Value::Object(map) => {
                if let Some(named) = map
                    .get("prefix")
                    .and_then(::serde_json::Value::as_str)
                    .and_then(S3Location::parse)
                {
                    prefix = named;
                }
            }
            ::serde_json::Value::String(suffix) => objects.push(S3Location {
                bucket: prefix.bucket.clone(),
                key: format!("{}{suffix}", prefix.key),
            }),
            _ => {}
        }
    }
    objects
}

/// The bytes of one object, up to `ceiling`.
///
/// `Err` is what Amazon S3 answered, as a failure reason's detail names it.
pub async fn read_object(
    client: &::aws_sdk_s3::Client,
    object: &S3Location,
    ceiling: usize,
) -> Result<Vec<u8>, String> {
    let read = client
        .get_object()
        .bucket(&object.bucket)
        .key(&object.key)
        // The ceiling is enforced by the REQUEST as well as by the bytes that arrive, so an
        // answer declaring no length is bounded too. An HTTP byte range is INCLUSIVE, so the
        // last byte asked for is `ceiling - 1`: the range states exactly the bound the
        // constant documents.
        .range(format!("bytes=0-{}", ceiling.saturating_sub(1)))
        .send()
        .await
        .map_err(|error| {
            ::tracing::debug!(
                bucket = ?object.bucket,
                key = ?object.key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a SageMaker training run could not read a training object"
            );
            s3_answer_detail(
                error
                    .as_service_error()
                    .and_then(::aws_sdk_s3::error::ProvideErrorMetadata::code),
                UNREAD_OBJECT_DETAIL,
            )
        })?;
    let body = read
        .body
        .collect()
        .await
        .map_err(|_| s3_answer_detail(None, UNREAD_OBJECT_DETAIL))?
        .into_bytes();
    Ok(body.into_iter().take(ceiling).collect())
}

/// Write the model archive as one object, or answer what Amazon S3 answered.
pub async fn write_object(
    client: &::aws_sdk_s3::Client,
    object: &S3Location,
    document: Vec<u8>,
) -> Result<(), String> {
    client
        .put_object()
        .bucket(&object.bucket)
        .key(&object.key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(document))
        .send()
        .await
        .map_err(|error| {
            ::tracing::debug!(
                bucket = ?object.bucket,
                key = ?object.key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a SageMaker training run could not write its model artifact"
            );
            s3_answer_detail(
                error
                    .as_service_error()
                    .and_then(::aws_sdk_s3::error::ProvideErrorMetadata::code),
                UNWRITTEN_OBJECT_DETAIL,
            )
        })?;
    Ok(())
}

/// Read the records one object holds into `read`, as comma-separated values.
///
/// A row whose first field is not a number, that holds no feature at all, or whose feature
/// count differs from the first record's, is COUNTED as skipped rather than guessed at: the
/// channel's form is the one the built-in algorithms document, and a row that is not of that
/// form is not a record of it.
pub fn read_records(bytes: &[u8], read: &mut ChannelData) {
    let expected = read.records.first().map(|record| record.features.len());
    let mut reader = ::csv::ReaderBuilder::new()
        // "SageMaker requires that a CSV file doesn't have a header record"
        // (`Channel$ContentType`'s `text/csv` form), so every row is a record.
        .has_headers(false)
        // A ragged row is the caller's data rather than a parse failure: it is counted as
        // skipped below.
        .flexible(true)
        .from_reader(bytes);
    let mut columns = expected;
    for row in reader.records() {
        if read.records.len() >= MAX_RECORDS {
            return;
        }
        let Ok(row) = row else {
            read.skipped += 1;
            continue;
        };
        let values: Option<Vec<f64>> = row
            .iter()
            .take(MAX_FEATURES + 1)
            .map(|field| field.trim().parse::<f64>().ok())
            .collect();
        let Some((target, features)) = values
            .as_deref()
            .and_then(<[f64]>::split_first)
            .filter(|(_, features)| !features.is_empty())
        else {
            read.skipped += 1;
            continue;
        };
        if columns.is_some_and(|held| held != features.len()) {
            read.skipped += 1;
            continue;
        }
        columns = Some(features.len());
        read.records.push(Record {
            target: *target,
            features: features.to_vec(),
        });
    }
}

/// Fit a least-squares linear model on `records` by batch gradient descent over `epochs`
/// passes.
///
/// The features are standardised for the descent — each column by its own mean and standard
/// deviation, so one step is comparable across columns whatever units they carry — and the
/// fitted weights are returned in the CALLER's scale, so the model predicts from the data as
/// it was given. A column that does not vary has no scale to divide by and takes no part.
pub fn fit(records: &[Record], epochs: usize) -> LinearModel {
    let columns = records.first().map_or(0, |record| record.features.len());
    let count = records.len().max(1) as f64;
    // The column sums and the squared deviations are accumulated by WALKING each record, so
    // nothing indexes into a record and a record of another width contributes only the
    // columns it has.
    let mut sums = vec![0.0_f64; columns];
    for record in records {
        for (slot, value) in sums.iter_mut().zip(&record.features) {
            *slot += value;
        }
    }
    let means: Vec<f64> = sums.iter().map(|sum| sum / count).collect();
    let mut squares = vec![0.0_f64; columns];
    for record in records {
        for ((slot, value), mean) in squares.iter_mut().zip(&record.features).zip(&means) {
            *slot += (value - mean).powi(2);
        }
    }
    let deviations: Vec<f64> = squares
        .iter()
        .map(|square| {
            let deviation = (square / count).sqrt();
            if deviation > f64::EPSILON {
                deviation
            } else {
                1.0
            }
        })
        .collect();
    // The standardised matrix is built ONCE, so the descent's passes cost arithmetic only.
    let standardised: Vec<Vec<f64>> = records
        .iter()
        .map(|record| {
            record
                .features
                .iter()
                .zip(&means)
                .zip(&deviations)
                .map(|((value, mean), deviation)| (value - mean) / deviation)
                .collect()
        })
        .collect();

    let mut weights = vec![0.0_f64; columns];
    let mut intercept = 0.0_f64;
    for _ in 0..epochs {
        let mut gradient = vec![0.0_f64; columns];
        let mut bias_gradient = 0.0_f64;
        for (row, record) in standardised.iter().zip(records) {
            let predicted = intercept
                + weights
                    .iter()
                    .zip(row)
                    .map(|(weight, value)| weight * value)
                    .sum::<f64>();
            let error = predicted - record.target;
            bias_gradient += error;
            for (slot, value) in gradient.iter_mut().zip(row) {
                *slot += error * value;
            }
        }
        intercept -= LEARNING_RATE * bias_gradient / count;
        for (weight, slot) in weights.iter_mut().zip(&gradient) {
            *weight -= LEARNING_RATE * slot / count;
        }
    }

    // Back to the caller's own scale: `w·(x - m)/s + b` is `(w/s)·x + (b - Σ w·m/s)`.
    let mut offset = intercept;
    let scaled: Vec<f64> = weights
        .iter()
        .zip(&means)
        .zip(&deviations)
        .map(|((weight, mean), deviation)| {
            offset -= weight * mean / deviation;
            weight / deviation
        })
        .collect();
    LinearModel {
        intercept: offset,
        weights: scaled,
        epochs,
    }
}

/// The model the archive holds: the parameters the fit measured, how to compute an inference
/// from them, and what the run read to produce them.
fn model_document(
    job_name: &str,
    request: &CreateTrainingJobInput,
    model: &LinearModel,
    channels: &[ChannelData],
    metrics: &[MetricData],
) -> ::serde_json::Value {
    let specification = request.algorithm_specification.as_ref();
    ::serde_json::json!({
        "trainingJobName": job_name,
        "algorithm": {
            "trainingImage": specification.and_then(|spec| spec.training_image.clone()),
            "algorithmName": specification.and_then(|spec| spec.algorithm_name.clone()),
        },
        "hyperParameters": request.hyper_parameters.clone().unwrap_or_default(),
        "model": {
            "type": "linear",
            "inference": "prediction = intercept + sum(weights[i] * feature[i])",
            "intercept": model.intercept,
            "weights": model.weights,
            "epochs": model.epochs,
        },
        "channels": channels
            .iter()
            .map(|read| {
                ::serde_json::json!({
                    "name": read.name,
                    "s3Uri": read.uri_read,
                    "objects": read.objects,
                    "records": read.records.len(),
                    "skippedRows": read.skipped,
                })
            })
            .collect::<Vec<_>>(),
        "metrics": metrics
            .iter()
            .map(|metric| {
                ::serde_json::json!({
                    "name": metric.metric_name,
                    "value": metric.value,
                })
            })
            .collect::<Vec<_>>(),
    })
}

/// The `model.tar.gz` bytes for `document`: a gzip-compressed tar holding the single
/// [`MODEL_ARCHIVE_ENTRY`] file, which is the form `ModelArtifacts` documents ("uploaded to
/// your Amazon S3 bucket as compressed files").
pub fn model_archive(document: &::serde_json::Value) -> ::std::io::Result<Vec<u8>> {
    use ::std::io::Write;

    let body = ::serde_json::to_vec_pretty(document)?;
    let encoder = ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    let mut archive = ::tar::Builder::new(encoder);
    let mut header = ::tar::Header::new_gnu();
    header.set_path(MODEL_ARCHIVE_ENTRY)?;
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    // The archive is a function of the fitted model alone: nothing about the machine or the
    // moment it was built takes part, so one model serializes to one archive.
    header.set_mtime(0);
    header.set_cksum();
    archive.append(&header, body.as_slice())?;
    let mut encoder = archive.into_inner()?;
    encoder.flush()?;
    encoder.finish()
}

// An Amazon S3 client over the in-process callback into this same emulator lives in
// `super::s3_access`, the one place this service builds one, and is reached here through
// `use super::*`.

/// The most bytes [`model_from_archive`] reads out of a model artifact AFTER decompression.
///
/// The ceilings on a read bound the COMPRESSED bytes only ([`MAX_OBJECT_BYTES`]), and a gzip
/// member expands by orders of magnitude, so the inflated side is bounded here too: the
/// decompressing stream and the entry read from it are both cut off at this many bytes, and an
/// archive that does not fit answers "no model" rather than growing the process. The budget is
/// generous against a model document this emulator writes — which is a few numbers per feature
/// column — and far below what the host would notice.
pub const MAX_INFLATED_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024;

/// The fitted model a `model.tar.gz` this emulator wrote holds, read back out of the archive.
///
/// The inverse of [`model_archive`]: the one reading of a model artifact, so whatever serves
/// an inference from a trained model reads the same parameters the fit measured instead of
/// deciding again how the archive is laid out. `None` for bytes that are not an archive this
/// emulator's own run produced — nothing is guessed for them.
///
/// The bytes are a caller's own object, so the DECOMPRESSED side is bounded as explicitly as
/// the download was: see [`MAX_INFLATED_ARCHIVE_BYTES`].
#[must_use]
pub fn model_from_archive(bytes: &[u8]) -> Option<LinearModel> {
    use ::std::io::Read;

    // The whole inflated stream is cut off at the budget, so no archive — however many
    // entries it declares and however large each says it is — can inflate past it.
    let decoder = ::flate2::read::GzDecoder::new(bytes).take(MAX_INFLATED_ARCHIVE_BYTES);
    let mut archive = ::tar::Archive::new(decoder);
    let mut document: Option<::serde_json::Value> = None;
    for entry in archive.entries().ok()? {
        let entry = entry.ok()?;
        let named = entry
            .path()
            .ok()
            .is_some_and(|path| path.as_os_str() == MODEL_ARCHIVE_ENTRY);
        if !named {
            continue;
        }
        // And the one entry the reading wants is cut off at the same budget, so the parse
        // that consumes it cannot be fed more than that either.
        document = ::serde_json::from_reader(entry.take(MAX_INFLATED_ARCHIVE_BYTES)).ok();
        break;
    }
    let model = document?.get("model")?.clone();
    let weights: Vec<f64> = model
        .get("weights")?
        .as_array()?
        .iter()
        .map(::serde_json::Value::as_f64)
        .collect::<Option<Vec<f64>>>()?;
    Some(LinearModel {
        intercept: model.get("intercept")?.as_f64()?,
        weights,
        epochs: model
            .get("epochs")
            .and_then(::serde_json::Value::as_u64)
            .unwrap_or(0) as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::{fit, fit_epochs, read_records, ChannelData, MAX_FEATURES, MAX_RECORDS};

    /// One channel's read, as the rows below drive it.
    fn read_of(bytes: &str) -> ChannelData {
        let mut read = ChannelData::new("train", "s3://bucket/train/".to_string());
        read_records(bytes.as_bytes(), &mut read);
        read
    }

    /// RULE: a training channel's records are the comma-separated rows whose first column is
    /// the target and whose remaining columns are the features; a row that is not of that
    /// form is counted, never guessed at.
    ///
    /// The rows differ along the record count, the feature count, the number form, the
    /// raggedness of the data and whether the row holds numbers at all.
    #[test]
    fn a_channels_records_are_its_comma_separated_rows() {
        let rows: [(&str, &str, usize, usize); 6] = [
            ("one record of one feature", "1,2\n", 1, 0),
            ("three records of two features", "1,2,3\n4,5,6\n7,8,9\n", 3, 0),
            ("a decimal and a negative", "1.5,-2.25\n-3,4e2\n", 2, 0),
            ("a header record", "target,feature\n1,2\n", 1, 1),
            ("a ragged row", "1,2\n3,4,5\n6,7\n", 2, 1),
            ("a row with no feature", "1\n2,3\n", 1, 1),
        ];
        for (case, body, records, skipped) in rows {
            let read = read_of(body);
            assert_eq!(
                (read.records.len(), read.skipped),
                (records, skipped),
                "case {case:?}: expected {records} record(s) and {skipped} skipped, got \
                 {:?}",
                read
            );
        }
    }

    /// RULE: a record wider than [`MAX_FEATURES`] is fitted on its first [`MAX_FEATURES`]
    /// features, and no more than [`MAX_RECORDS`] records are read.
    #[test]
    fn the_read_is_bounded_in_width_and_in_length() {
        let wide: String = (0..MAX_FEATURES + 8)
            .map(|column| column.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let read = read_of(&format!("{wide}\n"));
        let features = read.records.first().map_or(0, |record| record.features.len());
        assert_eq!(
            features, MAX_FEATURES,
            "case a record wider than the ceiling: expected {MAX_FEATURES} features, got \
             {features}"
        );

        let long: String = (0..MAX_RECORDS + 16)
            .map(|row| format!("{row},1\n"))
            .collect();
        let read = read_of(&long);
        assert_eq!(
            read.records.len(),
            MAX_RECORDS,
            "case more rows than the ceiling: expected {MAX_RECORDS} records, got {}",
            read.records.len()
        );
    }

    /// RULE: the fit MEASURES the model from the data — a linear relation in the records is
    /// recovered, so the error it reports falls as the fit explains the data.
    ///
    /// The rows differ along the relation the data carries: one feature, two features, a
    /// constant target and a column that does not vary at all.
    #[test]
    fn the_fit_recovers_a_linear_relation_in_the_records() {
        // Each row: the case, and the records whose relation the fit must recover.
        let rows: [(&str, &str); 4] = [
            ("one feature, y = 2x + 1", "3,1\n5,2\n7,3\n9,4\n"),
            ("two features, y = x1 + x2", "2,1,1\n4,1,3\n6,2,4\n8,3,5\n"),
            ("a constant target", "5,1\n5,2\n5,3\n"),
            ("a column that does not vary", "1,7\n3,7\n5,7\n"),
        ];
        for (case, body) in rows {
            let read = read_of(body);
            let columns = read
                .records
                .first()
                .map_or(0, |record| record.features.len());
            let model = fit(&read.records, fit_epochs(read.records.len(), columns));
            let error = model.rmse(&read.records);
            let spread = {
                let mean = read.records.iter().map(|record| record.target).sum::<f64>()
                    / read.records.len() as f64;
                (read.records
                    .iter()
                    .map(|record| (record.target - mean).powi(2))
                    .sum::<f64>()
                    / read.records.len() as f64)
                    .sqrt()
            };
            assert!(
                error <= spread + 1e-9,
                "case {case:?}: the fitted model's error {error} must not exceed the data's \
                 own spread {spread}"
            );
            assert!(
                model.weights.len() == columns && model.intercept.is_finite(),
                "case {case:?}: expected {columns} finite weights, got {model:?}"
            );
        }
    }

    /// RULE: the fit is bounded — the epochs it runs are at least one pass and never cost
    /// more than the operation budget admits.
    #[test]
    fn the_epoch_count_is_bounded_by_the_operation_budget() {
        for (case, records, columns) in [
            ("a tiny dataset", 4_usize, 1_usize),
            ("a wide dataset", MAX_RECORDS, MAX_FEATURES),
            ("no record at all", 0, 0),
        ] {
            let epochs = fit_epochs(records, columns);
            assert!(
                (1..=super::MAX_EPOCHS).contains(&epochs),
                "case {case:?}: expected between 1 and {} epochs, got {epochs}",
                super::MAX_EPOCHS
            );
            let cost = epochs * records * columns;
            assert!(
                cost <= super::MAX_FIT_OPERATIONS,
                "case {case:?}: expected at most {} (record, feature) updates, got {cost}",
                super::MAX_FIT_OPERATIONS
            );
        }
    }
}
