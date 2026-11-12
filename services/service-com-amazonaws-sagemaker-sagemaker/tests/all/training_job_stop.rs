// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `StopTrainingJob`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, and
//! every stop is proven by reading the job back through `DescribeTrainingJob`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: a stop of a
//! running job answers the operation's empty output and moves the stored job into the
//! transient `Stopping` status — whatever its name, Region or optional members — and the
//! CLOCK, not the read, owes it `Stopped`, with no failure reason beside a status the caller
//! asked for; a stop already in flight is not restarted; a job whose run is over has nothing
//! to stop; a name this account holds no job under resolves to no stop target and is refused
//! with HTTP 400 `ValidationException`; and a stop moves the one job it names and no other.
//!
//! This suite mounts SageMaker alone, so a job left to run cannot read its channels and
//! settles into the modelled `Failed` status (as `crate::training_job_admission` pins) —
//! which is one of the two ways a job reaches a terminal status here, the other being an
//! earlier stop.

use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator};
use crate::training_job_admission::{image_specification, training_body};

/// Long enough to pass `state::TRAINING_JOB_STOPS_AFTER` (300 ms).
const PAST_STOP_DUE: Duration =
    service_com_amazonaws_sagemaker_sagemaker::state::TRAINING_JOB_STOPS_AFTER
        .saturating_mul(2)
        .saturating_add(Duration::from_millis(100));

/// Long enough to pass `state::TRAINING_JOB_ENDS_AFTER`, after which a job left to
/// run has reached its terminal status.
const PAST_JOB_END_DUE: Duration =
    service_com_amazonaws_sagemaker_sagemaker::state::TRAINING_JOB_ENDS_AFTER
        .saturating_mul(2)
        .saturating_add(Duration::from_millis(100));

/// One row of the stop table: a case name, the job's name and Region, the output location
/// the create names, and whatever the request carries beyond the required members.
struct Row {
    case: &'static str,
    name: &'static str,
    region: &'static str,
    output: &'static str,
    members: Value,
}

/// The stop table. Rows differ along the axes the rule generalises over: the shape of the
/// job's name (`TrainingJobName` is `len 1..63` with pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` — smithy_query operation=StopTrainingJob — so the
/// rows sit on both published bounds and in between), the Region the caller signs for, how
/// the output location was written, and which optional members the create carried
/// (`HyperParameters`, `Environment`, `Tags`, `EnableNetworkIsolation`, `RetryStrategy`,
/// `ExperimentConfig`): none of them has any bearing on whether a running job can be
/// stopped.
fn stop_rows() -> Vec<Row> {
    vec![
        Row {
            case: "required members only, shortest name",
            // `TrainingJobName`'s minimum length of 1.
            name: "t",
            region: "us-east-1",
            output: "s3://demo/output/",
            members: json!({}),
        },
        Row {
            case: "hyphenated name, another region, no trailing separator on the output",
            name: "nightly-model-training-job",
            region: "eu-west-2",
            output: "s3://demo/nested/output",
            members: json!({
                "HyperParameters": { "eta": "0.2", "max_depth": "6" },
                "Environment": { "VERBOSE": "1" },
            }),
        },
        Row {
            case: "longest name, every optional block",
            // `TrainingJobName`'s maximum length of 63.
            name: "a234567890123456789012345678901234567890123456789012345678901b",
            region: "ap-southeast-1",
            output: "s3://demo/output/",
            members: json!({
                "Tags": [{ "Key": "team", "Value": "science" }],
                "EnableNetworkIsolation": true,
                "RetryStrategy": { "MaximumRetryAttempts": 2 },
                "ExperimentConfig": { "ExperimentName": "stop-experiment" },
            }),
        },
    ]
}

/// A `CreateTrainingJob` body for `row`: the shared required members plus whatever the row
/// adds on top of them.
fn create_body(row: &Row) -> Value {
    let mut body = training_body(row.name, image_specification(), row.output);
    for (member, value) in row.members.as_object().expect("the row is a JSON object") {
        body[member] = value.clone();
    }
    body
}

/// Seed one running job and answer the ARN the create minted.
async fn create_job(emulator: &Emulator, row: &Row) -> String {
    let (status, created) = emulator
        .call_in(row.region, "CreateTrainingJob", &create_body(row))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected CreateTrainingJob to answer 200, got {status} with {created:?}",
        row.case
    );
    str_field(&created, "TrainingJobArn")
}

/// `StopTrainingJob` for `name` in `region`, as the raw (status, body) pair, so a test can
/// assert on a refusal as well as on a stopped job.
async fn stop(emulator: &Emulator, region: &str, name: &str) -> (u16, Value) {
    emulator
        .call_in(region, "StopTrainingJob", &json!({ "TrainingJobName": name }))
        .await
}

/// `DescribeTrainingJob` for `name` in `region`, as the raw (status, body) pair.
async fn describe(emulator: &Emulator, region: &str, name: &str) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "DescribeTrainingJob",
            &json!({ "TrainingJobName": name }),
        )
        .await
}

/// RULE: a stop of a running training job is accepted with the operation's empty output and
/// moves the stored job into the transient `Stopping` status, with the matching secondary
/// status; the move to the terminal `Stopped` is owed by the CLOCK, carries no failure
/// reason — stopping is what the caller asked for — and is stable across later reads.
#[tokio::test]
async fn stopping_a_running_job_moves_it_to_stopping_then_stopped() {
    for row in stop_rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let created_arn = create_job(&emulator, &row).await;

        let (status, stopped) = stop(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StopTrainingJob to answer 200, got {status} \
             with {stopped:?}"
        );
        // `StopTrainingJob`'s output shape is `Unit` (smithy_query
        // operation=StopTrainingJob), so the accepted call carries no members.
        assert_eq!(
            stopped,
            json!({}),
            "case {case:?}: expected the stop's modelled empty output, got {stopped:?}"
        );

        // Straight after the stop the terminal move is not due yet, so the transient status
        // is observable — the one this operation's doc says a stop request puts the job in.
        let (status, stopping) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the stop to answer 200, got {status} \
             with {stopping:?}"
        );
        assert_eq!(
            (
                str_field(&stopping, "TrainingJobStatus"),
                str_field(&stopping, "SecondaryStatus"),
            ),
            ("Stopping".to_string(), "Stopping".to_string()),
            "case {case:?}: expected the transient Stopping status after the stop, \
             got {stopping:?}"
        );
        assert_eq!(
            str_field(&stopping, "TrainingJobArn"),
            created_arn,
            "case {case:?}: expected the stopped job to keep the ARN the create minted \
             ({created_arn}), got {stopping:?}"
        );
        assert!(
            stopping.get("FailureReason").is_none() && stopping.get("TrainingEndTime").is_none(),
            "case {case:?}: expected neither FailureReason nor TrainingEndTime while the \
             job is Stopping, got {stopping:?}"
        );

        tokio::time::sleep(PAST_STOP_DUE).await;

        let (status, after) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the stop fell due to answer 200, \
             got {status} with {after:?}"
        );
        assert_eq!(
            (
                str_field(&after, "TrainingJobStatus"),
                str_field(&after, "SecondaryStatus"),
            ),
            ("Stopped".to_string(), "Stopped".to_string()),
            "case {case:?}: expected the job the caller stopped to end in Stopped, \
             got {after:?}"
        );
        assert!(
            after.get("FailureReason").is_none(),
            "case {case:?}: expected no FailureReason beside a status the caller asked for, \
             got {after:?}"
        );
        let end_time = after
            .get("TrainingEndTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a TrainingEndTime in {after:?}"));
        let creation_time = after
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime in {after:?}"));
        assert!(
            end_time >= creation_time,
            "case {case:?}: expected TrainingEndTime {end_time} at or after CreationTime \
             {creation_time}, got {after:?}"
        );
        assert!(
            after.get("FinalMetricDataList").is_none(),
            "case {case:?}: expected no metric reported for a run the caller cut short, \
             got {after:?}"
        );

        // The move happened once: a later read reports exactly the same job, so the terminal
        // status and its timestamps are stable and no read re-stamps them.
        let (status, again) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated read to answer 200, got {status} \
             with {again:?}"
        );
        assert_eq!(
            again, after,
            "case {case:?}: expected the stopped job to be stable across reads, \
             got {again:?}"
        );
    }
}

/// RULE: the secondary-status history of a job being stopped says what the job's own
/// statuses say: the entry for the status it started in ends where the stopping entry
/// begins, and that entry stays open-ended until the job has ended.
#[tokio::test]
async fn the_status_history_of_a_stopped_job_follows_the_statuses_it_has_been_in() {
    for row in stop_rows() {
        let case = row.case;
        let emulator = Emulator::new();
        create_job(&emulator, &row).await;
        let (status, stopped) = stop(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the stop to answer 200, got {status} with {stopped:?}"
        );

        let (_, stopping) = describe(&emulator, row.region, row.name).await;
        let transitions = history(&stopping, case);
        assert_eq!(
            transitions.len(),
            2,
            "case {case:?}: expected the status the job left and the one it is in, \
             got {stopping:?}"
        );
        assert_eq!(
            transitions[0].get("EndTime"),
            transitions[1].get("StartTime"),
            "case {case:?}: expected the job to enter Stopping when it left its first \
             status, got {stopping:?}"
        );
        assert_eq!(
            transitions[1].get("Status"),
            stopping.get("SecondaryStatus"),
            "case {case:?}: expected the last entry to name the status the job is in, \
             got {stopping:?}"
        );
        assert!(
            transitions[1].get("EndTime").is_none(),
            "case {case:?}: expected the status a stopping job is still in not to have \
             ended, got {stopping:?}"
        );

        tokio::time::sleep(PAST_STOP_DUE).await;

        let (_, after) = describe(&emulator, row.region, row.name).await;
        let transitions = history(&after, case);
        assert_eq!(
            transitions.last().and_then(|entry| entry.get("Status")),
            after.get("SecondaryStatus"),
            "case {case:?}: expected the last entry to name the terminal status, \
             got {after:?}"
        );
        assert_eq!(
            transitions.last().and_then(|entry| entry.get("EndTime")),
            after.get("TrainingEndTime"),
            "case {case:?}: expected the terminal entry to end when the job ended, \
             got {after:?}"
        );
    }
}

/// A training job's `SecondaryStatusTransitions`, which every read of a job reports.
fn history(described: &Value, case: &str) -> Vec<Value> {
    described
        .get("SecondaryStatusTransitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected SecondaryStatusTransitions in {described:?}")
        })
        .clone()
}

/// RULE: a stop of a job whose stop is already in flight is accepted and changes nothing:
/// the job stays in `Stopping` and the span the first stop started is not restarted, so the
/// terminal status still falls due when that first stop made it due.
#[tokio::test]
async fn a_stop_already_in_flight_is_not_restarted() {
    for row in stop_rows() {
        let case = row.case;
        let emulator = Emulator::new();
        create_job(&emulator, &row).await;

        let (status, first) = stop(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first stop to answer 200, got {status} with {first:?}"
        );
        let (status, stopping) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the first stop to answer 200, \
             got {status} with {stopping:?}"
        );

        // Repeats while the stop is in flight answer the same way and leave the job where
        // the first stop put it, timestamps included.
        for repeat in 0..2 {
            let (status, again) = stop(&emulator, row.region, row.name).await;
            assert_eq!(
                status, 200,
                "case {case:?} repeat {repeat}: expected a repeated stop to answer 200, \
                 got {status} with {again:?}"
            );
            let (status, described) = describe(&emulator, row.region, row.name).await;
            assert_eq!(
                status, 200,
                "case {case:?} repeat {repeat}: expected the read to answer 200, \
                 got {status} with {described:?}"
            );
            assert_eq!(
                described, stopping,
                "case {case:?} repeat {repeat}: expected the repeated stop to leave the job \
                 exactly where the first stop put it, got {described:?}"
            );
        }

        tokio::time::sleep(PAST_STOP_DUE).await;

        let (status, after) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the stop fell due to answer 200, \
             got {status} with {after:?}"
        );
        assert_eq!(
            str_field(&after, "TrainingJobStatus"),
            "Stopped",
            "case {case:?}: expected the repeats not to push the terminal status past the \
             span the first stop started, got {after:?}"
        );
    }
}

/// RULE: a job that has already reached a status nothing leaves on its own has no run left
/// to stop: the call is refused with HTTP 400 `ValidationException` naming the job and the
/// status it found, and the stored job is left exactly as it was.
#[tokio::test]
async fn a_job_whose_run_is_over_has_nothing_to_stop() {
    // Rows differ along HOW the job reached a terminal status — left to run until the clock
    // ended it, or stopped by an earlier call — and along the job's name, Region, output
    // location and optional members.
    let rows = stop_rows();
    for (index, row) in rows.iter().enumerate() {
        // The first row is left to run; the rest are stopped first, so both ways into a
        // terminal status are covered over different names, Regions and member sets.
        let stop_first = index > 0;
        let case = row.case;
        let emulator = Emulator::new();
        create_job(&emulator, row).await;

        let expected_status = if stop_first {
            let (status, stopped) = stop(&emulator, row.region, row.name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the first stop to answer 200, got {status} \
                 with {stopped:?}"
            );
            tokio::time::sleep(PAST_STOP_DUE).await;
            "Stopped"
        } else {
            tokio::time::sleep(PAST_JOB_END_DUE).await;
            "Failed"
        };

        let (status, described) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer 200, got {status} with {described:?}"
        );
        assert_eq!(
            str_field(&described, "TrainingJobStatus"),
            expected_status,
            "case {case:?}: expected the job to have reached {expected_status} before the \
             stop under test, got {described:?}"
        );

        let (status, refused) = stop(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a stop of a {expected_status} job to be refused with \
             HTTP 400, got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected the refusal of a job that is found but not running to \
             be a ValidationException, got {refused:?}"
        );
        let refusal = message(&refused);
        assert!(
            refusal.contains(row.name) && refusal.contains(expected_status),
            "case {case:?}: expected the refusal to name the job {:?} and the status \
             {expected_status} it found, got {refusal:?}",
            row.name
        );

        // The refusal changed nothing: the job still reports what it reported before it.
        let (status, unchanged) = describe(&emulator, row.region, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the refusal to answer 200, got {status} \
             with {unchanged:?}"
        );
        assert_eq!(
            unchanged, described,
            "case {case:?}: expected the refused stop to leave the stored job untouched, \
             got {unchanged:?}"
        );
    }
}

/// RULE: a stop of a training job this account holds no record of resolves to no stop
/// target: it is refused with HTTP 400 `ValidationException` carrying the training-job
/// family's not-found sentence, and creates nothing.
#[tokio::test]
async fn a_name_that_names_no_job_is_refused_as_a_validation_exception() {
    // Rows differ along the ways a name can miss: never created, a prefix and a suffix of a
    // name that exists, and a different case of it — `TrainingJobName` is keyed as sent.
    let emulator = Emulator::new();
    let existing = Row {
        case: "the one stored job",
        name: "train-stop-present",
        region: "us-east-1",
        output: "s3://demo/output/",
        members: json!({}),
    };
    create_job(&emulator, &existing).await;

    let misses: [(&str, &str); 4] = [
        ("never created", "train-stop-absent"),
        ("a prefix of a stored name", "train-stop"),
        ("a suffix added to a stored name", "train-stop-present2"),
        ("a different case of a stored name", "TRAIN-STOP-PRESENT"),
    ];
    for (case, name) in misses {
        let (status, refused) = stop(&emulator, existing.region, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a stop of an unknown name to be refused with HTTP 400, \
             got {status} with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected a ValidationException for a stop target that does not \
             resolve, got {refused:?}"
        );
        // The sentence every training-job reader answers for a name it cannot resolve.
        assert_eq!(
            message(&refused),
            "Requested resource not found.",
            "case {case:?}: expected the training-job family's not-found sentence, \
             got {refused:?}"
        );

        // Nothing was created for the name the stop could not resolve.
        let (status, described) = describe(&emulator, existing.region, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the name to still resolve to no job, got {status} \
             with {described:?}"
        );
    }

    // And the job that does exist is untouched by every refusal above.
    let (status, present) = describe(&emulator, existing.region, existing.name).await;
    assert_eq!(
        status, 200,
        "case the one stored job: expected the read to answer 200, got {status} with \
         {present:?}"
    );
    assert_eq!(
        str_field(&present, "TrainingJobStatus"),
        "InProgress",
        "case the one stored job: expected the stored job to still be running, got {present:?}"
    );
}

/// RULE: a stop acts on the ONE job it names: every other stored job keeps the status it
/// had, whichever of them the caller stopped.
#[tokio::test]
async fn a_stop_moves_only_the_job_it_names() {
    let emulator = Emulator::new();
    let names = ["train-stop-a", "train-stop-b", "train-stop-c"];
    for name in names {
        create_job(
            &emulator,
            &Row {
                case: "one of three stored jobs",
                name,
                region: "us-east-1",
                output: "s3://demo/output/",
                members: json!({}),
            },
        )
        .await;
    }

    let stopped_name = names[1];
    let (status, stopped) = stop(&emulator, "us-east-1", stopped_name).await;
    assert_eq!(
        status, 200,
        "case {stopped_name}: expected the stop to answer 200, got {status} with {stopped:?}"
    );

    for name in names {
        let (status, described) = describe(&emulator, "us-east-1", name).await;
        assert_eq!(
            status, 200,
            "case {name}: expected the read to answer 200, got {status} with {described:?}"
        );
        let expected = if name == stopped_name {
            "Stopping"
        } else {
            "InProgress"
        };
        assert_eq!(
            str_field(&described, "TrainingJobStatus"),
            expected,
            "case {name}: expected {expected} after stopping {stopped_name}, got {described:?}"
        );
    }
}
