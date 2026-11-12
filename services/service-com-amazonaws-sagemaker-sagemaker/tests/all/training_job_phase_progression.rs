// SPDX-License-Identifier: CC0-1.0

//! How a training job's work PROGRESSES: the `SecondaryStatus` stages the job passes
//! through on its way to a terminal status, the history it reports for them, and the runtime
//! cap that can end it before they are over.
//!
//! Driven end to end through the same harness `training_job_execution` builds — SageMaker,
//! Amazon S3 and IAM on ONE shared engine, every call travelling the real wire — so what is
//! asserted is what an SDK client would see.
//!
//! RULE ONE — an accepted job whose run can read its channels and write its artifact
//! advances through the canonical secondary statuses
//! (`DescribeTrainingJob$SecondaryStatus`: `Starting`, then `Downloading` for a job with
//! data to bring down, `Training`, `Uploading`) and ends `Completed`/`Completed`, with
//! `SecondaryStatusTransitions` naming every stage it was in, in that order, each entry
//! starting where the one before it ended and carrying the `StatusMessage` that describes
//! that stage.
//!
//! RULE TWO — what fails a run is a LEG of its work that failed, not the SHAPE of the
//! caller's data: nothing in the API models what a channel's bytes mean, so data that holds
//! no record of the comma-separated form the built-in algorithms document still leaves a job
//! that completes, with the artifact written and no metric for a channel nothing was
//! measured over.
//!
//! RULE THREE — `StoppingCondition$MaxRuntimeInSeconds` is honoured: "if the job does not
//! complete during this time, SageMaker ends the job", which is the primary status `Stopped`
//! with the secondary `MaxRuntimeExceeded` beside it and no failure reason — the job ends
//! exactly at the cap — while a cap the work fits inside leaves the job `Completed`.
//!
//! The tables span what the rules generalise over: how many channels a job declares and
//! whether it declares any at all, the shape of the records they hold, and where a runtime
//! cap falls relative to the work — at the modelled minimum, inside the work, and at the
//! values the member's own doc names as its default and its maximum.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::training_job_execution::{
    artifact_key, artifact_uri, channel, csv_records, read_and_write, str_field, training_body,
    Emulator, BUCKET,
};

/// The `SecondaryStatus` members an accepted job passes through while it is `InProgress`,
/// in the order `DescribeTrainingJob$SecondaryStatus`'s own doc lists them
/// (smithy_query query=doc shape=DescribeTrainingJobResponse member=SecondaryStatus), and
/// then the terminal members this crate's jobs end in.
const CANONICAL_ORDER: [&str; 7] = [
    "Starting",
    "Downloading",
    "Training",
    "Uploading",
    "Completed",
    "MaxRuntimeExceeded",
    "Stopped",
];

/// The stages of its work a run brings data down for: what RULE ONE's history must name for
/// a job whose channels name an Amazon S3 location.
const WITH_DOWNLOAD: [&str; 4] = ["Starting", "Downloading", "Training", "Uploading"];

/// The same for a job that declares no channel at all: there is nothing to download, which
/// `DescribeTrainingJob$SecondaryStatus` calls "an optional stage".
const WITHOUT_DOWNLOAD: [&str; 3] = ["Starting", "Training", "Uploading"];

/// The statuses nothing leaves on its own, so a poll that reaches one is over.
const TERMINAL: [&str; 3] = ["Completed", "Failed", "Stopped"];

/// How long a poll below gives the clock to settle a job: far longer than the span the state
/// gives a job's work, so a loaded host still settles well inside it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// How often a poll looks again. Only the loop's condition decides when it stops.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Describe `job` once.
async fn describe(emulator: &Emulator, job: &str) -> Value {
    emulator
        .sagemaker_ok("DescribeTrainingJob", &json!({ "TrainingJobName": job }))
        .await
}

/// Poll `DescribeTrainingJob` until `job` reaches a status nothing leaves on its own,
/// answering that last response and every distinct `SecondaryStatus` observed on the way.
async fn poll_until_terminal(emulator: &Emulator, job: &str, case: &str) -> (Value, Vec<String>) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let mut observed: Vec<String> = Vec::new();
    loop {
        let described = describe(emulator, job).await;
        let secondary = str_field(&described, "SecondaryStatus");
        if observed.last() != Some(&secondary) {
            observed.push(secondary);
        }
        if TERMINAL.contains(&str_field(&described, "TrainingJobStatus").as_str()) {
            return (described, observed);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job {job} must reach a terminal status, last read {described}, \
             having been in {observed:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `SecondaryStatusTransitions` a read reports, as (status, start, end, message) rows.
fn history(described: &Value, case: &str) -> Vec<(String, f64, Option<f64>, Option<String>)> {
    described
        .get("SecondaryStatusTransitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected SecondaryStatusTransitions in {described}"))
        .iter()
        .map(|entry| {
            (
                entry
                    .get("Status")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                entry
                    .get("StartTime")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| panic!("case {case:?}: an entry with no StartTime in {described}")),
                entry.get("EndTime").and_then(Value::as_f64),
                entry
                    .get("StatusMessage")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .collect()
}

/// Where `status` sits in the canonical order, so a sequence of observations can be weighed
/// against it.
fn order_of(status: &str, case: &str) -> usize {
    CANONICAL_ORDER
        .iter()
        .position(|known| *known == status)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {status:?} is not one of the modelled statuses a job reaches")
        })
}

/// RULE ONE and RULE TWO: an accepted job advances through the canonical stages of its work
/// to `Completed`, and reports the history of the stages it was in.
///
/// The rows differ along how many channels the job declares, whether it declares any at all,
/// and the shape of the records they hold.
#[tokio::test]
async fn an_accepted_job_advances_through_the_canonical_stages_of_its_work() {
    /// One case: the job's name, the channels it declares (none at all when `None`), the
    /// stages its work must pass through, and whether the run measured a metric.
    struct Row {
        case: &'static str,
        job: &'static str,
        channels: Option<Value>,
        stages: &'static [&'static str],
        measured: bool,
    }

    let emulator = Emulator::new();
    emulator.given_bucket(BUCKET).await;
    let role = emulator
        .given_role("phase-progression-role", Some(&read_and_write()))
        .await;
    emulator
        .given_object(BUCKET, "phases/train.csv", &csv_records(6, 2))
        .await;
    emulator
        .given_object(BUCKET, "phases/validation.csv", &csv_records(3, 2))
        .await;
    // Records of a form the comma-separated reading does not hold: one column only, and
    // fields that are not numbers. The run reads neither as a record.
    emulator
        .given_object(BUCKET, "phases/one-column.csv", "1\n2\n3\n")
        .await;
    emulator
        .given_object(BUCKET, "phases/notes.csv", "alpha,beta\ngamma,delta\n")
        .await;

    let rows = vec![
        Row {
            case: "one channel of comma-separated records",
            job: "phases-one-channel",
            channels: Some(json!([channel(
                "train",
                &format!("s3://{BUCKET}/phases/train.csv"),
                "S3Prefix"
            )])),
            stages: &WITH_DOWNLOAD,
            measured: true,
        },
        Row {
            case: "a training channel beside a validation channel",
            job: "phases-two-channels",
            channels: Some(json!([
                channel(
                    "train",
                    &format!("s3://{BUCKET}/phases/train.csv"),
                    "S3Prefix"
                ),
                channel(
                    "validation",
                    &format!("s3://{BUCKET}/phases/validation.csv"),
                    "S3Prefix"
                ),
            ])),
            stages: &WITH_DOWNLOAD,
            measured: true,
        },
        Row {
            case: "no channel at all, so nothing to download",
            job: "phases-no-channel",
            channels: None,
            stages: &WITHOUT_DOWNLOAD,
            measured: false,
        },
        Row {
            case: "records of one column, which hold no record of the channel's form",
            job: "phases-one-column",
            channels: Some(json!([channel(
                "train",
                &format!("s3://{BUCKET}/phases/one-column.csv"),
                "S3Prefix"
            )])),
            stages: &WITH_DOWNLOAD,
            measured: false,
        },
        Row {
            case: "fields that are not numbers",
            job: "phases-not-numbers",
            channels: Some(json!([channel(
                "train",
                &format!("s3://{BUCKET}/phases/notes.csv"),
                "S3Prefix"
            )])),
            stages: &WITH_DOWNLOAD,
            measured: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let output = format!("s3://{BUCKET}/phase-output/{}/", row.job);
        let mut body = training_body(
            row.job,
            &role,
            row.channels.clone().unwrap_or_else(|| json!([])),
            &output,
        );
        if row.channels.is_none() {
            body.as_object_mut()
                .expect("the request body is an object")
                .remove("InputDataConfig");
        }
        emulator.sagemaker_ok("CreateTrainingJob", &body).await;

        // The job is accepted with no stage of its work begun, and the move out of that is
        // the clock's — so a read straight after the create observes it.
        let accepted = describe(&emulator, row.job).await;
        assert_eq!(
            (
                str_field(&accepted, "TrainingJobStatus"),
                str_field(&accepted, "SecondaryStatus"),
            ),
            ("InProgress".to_string(), "Starting".to_string()),
            "case {case:?}: expected the accepted job to be InProgress/Starting, \
             got {accepted}"
        );

        let (settled, observed) = poll_until_terminal(&emulator, row.job, case).await;
        assert_eq!(
            (
                str_field(&settled, "TrainingJobStatus"),
                str_field(&settled, "SecondaryStatus"),
            ),
            ("Completed".to_string(), "Completed".to_string()),
            "case {case:?}: expected a run that read its channels and wrote its artifact to \
             complete, got {settled}"
        );
        assert!(
            settled.get("FailureReason").is_none(),
            "case {case:?}: expected no FailureReason beside a completed job, got {settled}"
        );

        // Every status a poller saw is in the canonical order, and the stages between the
        // accepted job and its terminal status are observable: the span the clock gives each
        // one is many times the interval this loop looks again on.
        let mut reached = 0;
        for status in &observed {
            let position = order_of(status, case);
            assert!(
                position >= reached,
                "case {case:?}: {status:?} is out of the canonical order, having been in \
                 {observed:?}"
            );
            reached = position;
        }
        assert!(
            observed
                .iter()
                .any(|status| ["Downloading", "Training", "Uploading"].contains(&status.as_str())),
            "case {case:?}: expected a poller to observe a stage of the work in progress, \
             having been in {observed:?}"
        );

        // The history names every stage the job was in, in the canonical order, each entry
        // beginning where the one before it ended, and the last one ending when the job did.
        let transitions = history(&settled, case);
        let expected: Vec<String> = row
            .stages
            .iter()
            .map(|stage| (*stage).to_string())
            .chain(["Completed".to_string()])
            .collect();
        assert_eq!(
            transitions
                .iter()
                .map(|(status, ..)| status.clone())
                .collect::<Vec<_>>(),
            expected,
            "case {case:?}: expected the history to name the stages of the work, got {settled}"
        );
        for (index, pair) in transitions.windows(2).enumerate() {
            assert_eq!(
                pair[0].2,
                Some(pair[1].1),
                "case {case:?}: entry {index} must end where the next one begins, got {settled}"
            );
        }
        for (status, _, _, message) in &transitions {
            assert!(
                message.as_deref().is_some_and(|text| !text.is_empty()),
                "case {case:?}: the {status} entry must describe the progress within it, \
                 got {settled}"
            );
        }
        let end_time = settled.get("TrainingEndTime").and_then(Value::as_f64);
        assert_eq!(
            transitions.last().and_then(|(_, _, end, _)| *end),
            end_time,
            "case {case:?}: the last entry must end when the job ended, got {settled}"
        );

        // The artifact the run uploaded is an object Amazon S3 holds at the URI the describe
        // reports, whatever the records it fitted on.
        let artifact = settled
            .get("ModelArtifacts")
            .and_then(|artifacts| artifacts.get("S3ModelArtifacts"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            artifact,
            artifact_uri(&output, row.job),
            "case {case:?}: expected the artifact under the declared output path, got {settled}"
        );
        assert!(
            emulator
                .object_bytes(BUCKET, &artifact_key(&output, row.job))
                .await
                .is_some(),
            "case {case:?}: expected {artifact} to name an object Amazon S3 holds"
        );
        assert_eq!(
            settled.get("FinalMetricDataList").is_some(),
            row.measured,
            "case {case:?}: a metric is reported for the channels the run measured one over \
             and for no others, got {settled}"
        );
    }
}

/// RULE THREE: a runtime cap the job's work does not fit inside ends it at the cap, with the
/// `Stopped`/`MaxRuntimeExceeded` pair the model groups together and no failure reason; a cap
/// the work fits inside leaves the job `Completed`.
///
/// The rows differ along where the cap falls: at the modelled minimum, inside the work, and
/// at the two values `StoppingCondition$MaxRuntimeInSeconds`'s own doc names — "the default
/// value is 1 day", "the maximum value is 28 days" — both of which the work fits well
/// inside, plus a job that states no stopping condition at all.
#[tokio::test]
async fn a_runtime_cap_the_work_does_not_fit_inside_ends_the_job_at_the_cap() {
    let emulator = Emulator::new();
    emulator.given_bucket(BUCKET).await;
    let role = emulator
        .given_role("runtime-cap-role", Some(&read_and_write()))
        .await;
    emulator
        .given_object(BUCKET, "caps/train.csv", &csv_records(5, 2))
        .await;

    // Each row: the case, the job's name, the cap it states (none at all for `None`), and the
    // (TrainingJobStatus, SecondaryStatus) pair it must end in.
    let rows: [(&str, &str, Option<i64>, &str, &str); 5] = [
        (
            "the modelled minimum cap",
            "cap-minimum",
            Some(1),
            "Stopped",
            "MaxRuntimeExceeded",
        ),
        (
            "a cap inside the span of the work",
            "cap-inside-the-work",
            Some(5),
            "Stopped",
            "MaxRuntimeExceeded",
        ),
        (
            "the cap the member documents as its default",
            "cap-one-day",
            Some(86_400),
            "Completed",
            "Completed",
        ),
        (
            "the cap the member documents as its maximum",
            "cap-28-days",
            Some(2_419_200),
            "Completed",
            "Completed",
        ),
        (
            "no stopping condition at all",
            "cap-absent",
            None,
            "Completed",
            "Completed",
        ),
    ];

    for (case, job, cap, status, secondary) in rows {
        let output = format!("s3://{BUCKET}/cap-output/{job}/");
        let mut body = training_body(
            job,
            &role,
            json!([channel(
                "train",
                &format!("s3://{BUCKET}/caps/train.csv"),
                "S3Prefix"
            )]),
            &output,
        );
        match cap {
            Some(seconds) => body["StoppingCondition"] = json!({ "MaxRuntimeInSeconds": seconds }),
            None => {
                body.as_object_mut()
                    .expect("the request body is an object")
                    .remove("StoppingCondition");
            }
        }
        emulator.sagemaker_ok("CreateTrainingJob", &body).await;

        let (settled, observed) = poll_until_terminal(&emulator, job, case).await;
        assert_eq!(
            (
                str_field(&settled, "TrainingJobStatus"),
                str_field(&settled, "SecondaryStatus"),
            ),
            (status.to_string(), secondary.to_string()),
            "case {case:?}: expected the job to end {status}/{secondary}, got {settled}, \
             having been in {observed:?}"
        );
        assert!(
            settled.get("FailureReason").is_none(),
            "case {case:?}: a job the cap ended carries no failure reason, got {settled}"
        );
        let last = history(&settled, case)
            .pop()
            .unwrap_or_else(|| panic!("case {case:?}: expected a history entry in {settled}"));
        assert_eq!(
            last.0, secondary,
            "case {case:?}: the last history entry names the status the job ended in, \
             got {settled}"
        );

        // A job the cap ended ends exactly at the cap, measured from the instant it was
        // created: the seconds the request asked for and no more.
        let created_at = settled
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime in {settled}"));
        let ended_at = settled
            .get("TrainingEndTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a TrainingEndTime in {settled}"));
        if secondary == "MaxRuntimeExceeded" {
            let cap = cap.expect("a capped case states its cap") as f64;
            assert!(
                (ended_at - created_at - cap).abs() < 0.001,
                "case {case:?}: expected the job to end {cap} seconds after it was created, \
                 got {settled}"
            );
        } else {
            assert!(
                ended_at > created_at,
                "case {case:?}: expected the job to end after it was created, got {settled}"
            );
        }
    }
}
