// SPDX-License-Identifier: CC0-1.0

//! What an AutoML job V2's RUN is: the dataset it reads out of its channels, the model
//! candidates it fits on them, the metrics it measures each candidate by, the candidate it
//! picks as best, and the artifacts and notebooks it writes under
//! `OutputDataConfig.S3OutputPath`.
//!
//! Driven end to end with the emulator's own Amazon S3 and IAM mounted beside SageMaker on
//! ONE shared engine — the harness `training_job_execution` already defines for this target —
//! so every case reads and writes its objects in the service that OWNS objects and is
//! authorized against the role records IAM itself wrote.
//!
//! RULE: an accepted `CreateAutoMLJobV2` progresses on its own, with no further call from the
//! client: the clock moves it out of `InProgress` through the stages of its work into
//! `Completed`, and `DescribeAutoMLJobV2` then reports a `BestCandidate` whose
//! `FinalAutoMLJobObjectiveMetric` was MEASURED over the records held back from the fit, the
//! `AutoMLJobArtifacts` notebooks as objects Amazon S3 really holds, and the
//! `AutoMLResolvedAttributes` its analysis resolved — with `FailureReason` absent.
//!
//! The table's rows differ along what the rule generalises over: how the channel's location
//! is addressed (one object, a prefix holding several), how many records and feature columns
//! the data carries, which `ProblemType` the request names and whether it names one at all
//! (so the run resolves it from the target column instead), which objective metric it asks
//! for and whether it asks for one, how the validation records are chosen (a `validation`
//! channel, `DataSplitConfig.ValidationFraction`, neither), and whether the output path
//! carries a trailing separator.
//!
//! A second rule is asserted beside it: the measurements are COMPUTED from the caller's own
//! data, so two jobs over different datasets do not report the same objective value.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::training_job_execution::{
    csv_records, read_and_write, str_field, Emulator, BUCKET,
};

/// How long a job is polled for its terminal status before the case fails, and how often.
///
/// What the poll waits for is the CONDITION — the job reaching a status nothing leaves on its
/// own — re-read in a loop, so a loaded host does not fail a case the clock would have served.
/// The budget is far longer than the span the state gives an AutoML job's work
/// (`state::AUTO_ML_JOB_TERMINAL_AFTER`).
const TERMINAL_BUDGET: Duration = Duration::from_secs(90);
const TERMINAL_POLL: Duration = Duration::from_millis(50);

/// `AutoMLJobStatus`'s members nothing leaves on its own
/// (`smithy_query(query="shape",shape="AutoMLJobStatus")`).
const TERMINAL_STATUSES: &[&str] = &["Completed", "Failed", "Stopped"];

/// How many candidates one run fits, i.e. the length of
/// `state::CANDIDATE_EPOCH_BUDGETS`, read from the constant so the expectation tracks it.
const CANDIDATES: usize =
    service_com_amazonaws_sagemaker_sagemaker::state::CANDIDATE_EPOCH_BUDGETS.len();

/// One row of the table.
struct Row {
    case: &'static str,
    /// The objects the channels are staged from, as (key, bytes) pairs.
    objects: Vec<(String, String)>,
    /// The `AutoMLJobInputDataConfig` the request carries.
    channels: Value,
    /// The members the request's `TabularJobConfig` carries beyond `TargetAttributeName`.
    tabular: Value,
    /// The request's `AutoMLJobObjective`, when it names one.
    objective: Option<Value>,
    /// The request's `DataSplitConfig`, when it names one.
    data_split: Option<Value>,
    /// The output location the job writes under.
    output: String,
    /// The `ProblemType` the run must resolve.
    resolved_problem_type: &'static str,
    /// The `AutoMLMetricEnum` member the candidates must be ranked by.
    objective_metric: &'static str,
    /// The `AutoMLJobObjectiveType` that metric is better in.
    objective_direction: &'static str,
}

/// One tabular channel addressing `uri` as `data_type`, of `channel_type`.
fn channel(channel_type: &str, uri: &str, data_type: &str) -> Value {
    json!({
        "ChannelType": channel_type,
        "ContentType": "text/csv;header=present",
        "DataSource": { "S3DataSource": { "S3DataType": data_type, "S3Uri": uri } },
    })
}

/// Comma-separated records whose first column is a CLASS LABEL and whose remaining columns
/// are features: `rows` records over `classes` labels, the label a function of the features,
/// so a fitted model can be measured as a classifier over real predictions.
fn csv_classes(rows: usize, classes: i64) -> String {
    (0..rows)
        .map(|row| {
            let label = (row as i64) % classes;
            let first = label as f64 * 10.0 + (row % 3) as f64;
            let second = label as f64 * 10.0 - (row % 2) as f64;
            format!("{label},{first},{second}\n")
        })
        .collect()
}

/// The table. Every row's dataset holds records of the tabular form, so no row can pass by
/// reading nothing.
///
/// `AutoMLJobObjective` and `TabularJobConfig.ProblemType` are documented as given together
/// or not at all, so a row either names both — any of the metrics its problem type lists — or
/// names neither and leaves the run to resolve both from the data.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one object, regression named with its objective, default split",
            objects: vec![("tab/one/data.csv".to_string(), csv_records(40, 3))],
            channels: json!([channel(
                "training",
                &format!("s3://{BUCKET}/tab/one/data.csv"),
                "S3Prefix"
            )]),
            tabular: json!({ "ProblemType": "Regression" }),
            objective: Some(json!({ "MetricName": "MSE" })),
            data_split: None,
            output: format!("s3://{BUCKET}/automl/one/"),
            resolved_problem_type: "Regression",
            objective_metric: "MSE",
            objective_direction: "Minimize",
        },
        Row {
            case: "a prefix holding several objects, nothing named, inferred regression",
            objects: vec![
                ("tab/many/a.csv".to_string(), csv_records(25, 2)),
                ("tab/many/b.csv".to_string(), csv_records(15, 2)),
            ],
            channels: json!([channel(
                "training",
                &format!("s3://{BUCKET}/tab/many/"),
                "S3Prefix"
            )]),
            tabular: json!({}),
            objective: None,
            data_split: None,
            // No trailing separator on the output path: the run's own keys are laid out
            // under it either way.
            output: format!("s3://{BUCKET}/automl/many"),
            resolved_problem_type: "Regression",
            // The documented default objective metric for a regression problem type.
            objective_metric: "MSE",
            objective_direction: "Minimize",
        },
        Row {
            case: "regression ranked by RMSE with a validation fraction",
            objects: vec![("tab/split/data.csv".to_string(), csv_records(60, 4))],
            channels: json!([channel(
                "training",
                &format!("s3://{BUCKET}/tab/split/data.csv"),
                "S3Prefix"
            )]),
            tabular: json!({ "ProblemType": "Regression" }),
            objective: Some(json!({ "MetricName": "RMSE" })),
            data_split: Some(json!({ "ValidationFraction": 0.5 })),
            output: format!("s3://{BUCKET}/automl/split/"),
            resolved_problem_type: "Regression",
            objective_metric: "RMSE",
            objective_direction: "Minimize",
        },
        Row {
            case: "binary classification with its own validation channel",
            objects: vec![
                ("tab/binary/train.csv".to_string(), csv_classes(40, 2)),
                ("tab/binary/valid.csv".to_string(), csv_classes(12, 2)),
            ],
            channels: json!([
                channel(
                    "training",
                    &format!("s3://{BUCKET}/tab/binary/train.csv"),
                    "S3Prefix"
                ),
                channel(
                    "validation",
                    &format!("s3://{BUCKET}/tab/binary/valid.csv"),
                    "S3Prefix"
                ),
            ]),
            tabular: json!({ "ProblemType": "BinaryClassification" }),
            objective: Some(json!({ "MetricName": "Accuracy" })),
            data_split: None,
            output: format!("s3://{BUCKET}/automl/binary/"),
            resolved_problem_type: "BinaryClassification",
            objective_metric: "Accuracy",
            objective_direction: "Maximize",
        },
        Row {
            case: "nothing named over three class labels, inferred multiclass",
            objects: vec![("tab/inferred/data.csv".to_string(), csv_classes(45, 3))],
            channels: json!([channel(
                "training",
                &format!("s3://{BUCKET}/tab/inferred/data.csv"),
                "S3Prefix"
            )]),
            tabular: json!({}),
            objective: None,
            data_split: None,
            output: format!("s3://{BUCKET}/automl/inferred/"),
            resolved_problem_type: "MulticlassClassification",
            // The documented default objective metric for a multiclass problem type.
            objective_metric: "Accuracy",
            objective_direction: "Maximize",
        },
        Row {
            case: "multiclass classification ranked by a macro-averaged metric",
            objects: vec![("tab/multi/data.csv".to_string(), csv_classes(45, 3))],
            channels: json!([channel(
                "training",
                &format!("s3://{BUCKET}/tab/multi/data.csv"),
                "S3Prefix"
            )]),
            tabular: json!({ "ProblemType": "MulticlassClassification" }),
            objective: Some(json!({ "MetricName": "BalancedAccuracy" })),
            data_split: None,
            output: format!("s3://{BUCKET}/automl/multi/"),
            resolved_problem_type: "MulticlassClassification",
            objective_metric: "BalancedAccuracy",
            objective_direction: "Maximize",
        },
        Row {
            case: "a completion criteria naming its own candidate cap",
            objects: vec![("tab/cap/data.csv".to_string(), csv_records(30, 2))],
            channels: json!([channel(
                "training",
                &format!("s3://{BUCKET}/tab/cap/data.csv"),
                "S3Prefix"
            )]),
            tabular: json!({
                "ProblemType": "Regression",
                "CompletionCriteria": { "MaxCandidates": 7 },
            }),
            objective: Some(json!({ "MetricName": "R2" })),
            data_split: None,
            output: format!("s3://{BUCKET}/automl/cap/"),
            resolved_problem_type: "Regression",
            objective_metric: "R2",
            objective_direction: "Maximize",
        },
    ]
}

/// A `CreateAutoMLJobV2` body for `row`.
fn create_body(name: &str, role_arn: &str, row: &Row) -> Value {
    let mut tabular = json!({ "TargetAttributeName": "y" });
    let object = tabular.as_object_mut().expect("a JSON object");
    for (member, value) in row.tabular.as_object().expect("a JSON object") {
        object.insert(member.clone(), value.clone());
    }
    let mut body = json!({
        "AutoMLJobName": name,
        "AutoMLJobInputDataConfig": row.channels.clone(),
        "OutputDataConfig": { "S3OutputPath": row.output.clone() },
        "AutoMLProblemTypeConfig": { "TabularJobConfig": tabular },
        "RoleArn": role_arn,
    });
    let request = body.as_object_mut().expect("a JSON object");
    if let Some(objective) = row.objective.clone() {
        request.insert("AutoMLJobObjective".to_string(), objective);
    }
    if let Some(split) = row.data_split.clone() {
        request.insert("DataSplitConfig".to_string(), split);
    }
    body
}

/// Poll `DescribeAutoMLJobV2` until the job reaches a status nothing leaves on its own,
/// answering that read. Waits for the CONDITION, never for a fixed span.
async fn settled(emulator: &Emulator, case: &str, name: &str) -> Value {
    let deadline = Instant::now() + TERMINAL_BUDGET;
    loop {
        let described = emulator
            .sagemaker_ok("DescribeAutoMLJobV2", &json!({ "AutoMLJobName": name }))
            .await;
        let status = str_field(&described, "AutoMLJobStatus");
        if TERMINAL_STATUSES.contains(&status.as_str()) {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the job never reached a terminal status; last read {described}"
        );
        tokio::time::sleep(TERMINAL_POLL).await;
    }
}

/// The key of an object of [`BUCKET`] named by `uri`.
fn key_of(uri: &str) -> String {
    uri.strip_prefix(&format!("s3://{BUCKET}/"))
        .unwrap_or_else(|| panic!("{uri} is not an object of the harness bucket"))
        .to_string()
}

/// A nested string member, by JSON pointer.
fn pointer_str(body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string at {pointer} in {body}"))
        .to_string()
}

/// RULE: an accepted AutoML job V2 runs itself to `Completed` and reports the candidate it
/// picked, the metrics it measured, the notebooks it wrote and the attributes it resolved.
#[tokio::test]
async fn an_accepted_job_runs_itself_to_completed_and_reports_what_it_produced() {
    for row in rows() {
        let emulator = Emulator::new();
        emulator.given_bucket(BUCKET).await;
        for (key, body) in &row.objects {
            emulator.given_object(BUCKET, key, body).await;
        }
        let role_arn = emulator
            .given_role("automl-run-role", Some(&read_and_write()))
            .await;
        let case = row.case;
        let name = "automl-run";

        emulator
            .sagemaker_ok("CreateAutoMLJobV2", &create_body(name, &role_arn, &row))
            .await;
        let settled = settled(&emulator, case, name).await;

        assert_eq!(
            str_field(&settled, "AutoMLJobStatus"),
            "Completed",
            "case {case:?}: an accepted job must run to Completed, got {settled}"
        );
        assert_eq!(
            str_field(&settled, "AutoMLJobSecondaryStatus"),
            "Completed",
            "case {case:?}: AutoMLJobSecondaryStatus, got {settled}"
        );
        assert!(
            settled.get("FailureReason").is_none(),
            "case {case:?}: a completed job reports no FailureReason, got {settled}"
        );

        // The candidate the objective metric picked is one of the candidates the run fitted,
        // named for this job.
        let candidate = pointer_str(&settled, "/BestCandidate/CandidateName");
        let expected: Vec<String> = (1..=CANDIDATES)
            .map(|index| format!("{name}-candidate-{index}"))
            .collect();
        assert!(
            expected.contains(&candidate),
            "case {case:?}: BestCandidate must be one of {expected:?}, got {candidate:?}"
        );
        assert_eq!(
            pointer_str(&settled, "/BestCandidate/CandidateStatus"),
            "Completed",
            "case {case:?}: the best candidate's status, got {settled}"
        );
        assert_eq!(
            pointer_str(&settled, "/BestCandidate/ObjectiveStatus"),
            "Succeeded",
            "case {case:?}: the best candidate's objective status, got {settled}"
        );

        // The objective metric: the one the request asked for when this run measures it, and
        // otherwise the documented default for the resolved problem type — measured, so the
        // value is a real number.
        let metric = settled
            .pointer("/BestCandidate/FinalAutoMLJobObjectiveMetric")
            .unwrap_or_else(|| panic!("case {case:?}: no objective metric in {settled}"));
        assert_eq!(
            (
                str_field(metric, "MetricName"),
                str_field(metric, "Type"),
                str_field(metric, "StandardMetricName"),
            ),
            (
                row.objective_metric.to_string(),
                row.objective_direction.to_string(),
                row.objective_metric.to_string(),
            ),
            "case {case:?}: the objective metric the candidates were ranked by, got {metric}"
        );
        let value = metric
            .get("Value")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: no metric Value in {metric}"));
        assert!(
            value.is_finite(),
            "case {case:?}: the objective metric must be a measured number, got {value}"
        );

        // Every metric the resolved problem type lists is reported for the candidate,
        // measured over the records held back from the fit.
        let measured = settled
            .pointer("/BestCandidate/CandidateProperties/CandidateMetrics")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no candidate metrics in {settled}"));
        assert!(
            measured
                .iter()
                .any(|datum| str_field(datum, "MetricName") == row.objective_metric),
            "case {case:?}: the candidate's metrics must include {}, got {measured:?}",
            row.objective_metric
        );
        for datum in measured {
            assert_eq!(
                str_field(datum, "Set"),
                "Validation",
                "case {case:?}: a candidate metric is measured over the held-back records, \
                 got {datum}"
            );
        }

        // Both notebooks the job reports as its artifacts are objects Amazon S3 holds.
        for member in [
            "DataExplorationNotebookLocation",
            "CandidateDefinitionNotebookLocation",
        ] {
            let uri = pointer_str(&settled, &format!("/AutoMLJobArtifacts/{member}"));
            let held = emulator.object_bytes(BUCKET, &key_of(&uri)).await;
            let bytes = held.unwrap_or_else(|| {
                panic!("case {case:?}: Amazon S3 holds no object at {uri} for {member}")
            });
            let document: Value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!("case {case:?}: {member} at {uri} is not a notebook document: {error}")
            });
            assert!(
                document.get("cells").and_then(Value::as_array).is_some(),
                "case {case:?}: {member} at {uri} carries no cells, got {document}"
            );
        }

        // The model each candidate fitted was written under the job's own output location.
        let prefix = row.output.trim_end_matches('/');
        let artifact =
            format!("{prefix}/{name}/sagemaker-automl-candidates/{candidate}/output/model.tar.gz");
        assert!(
            emulator
                .object_bytes(BUCKET, &key_of(&artifact))
                .await
                .is_some(),
            "case {case:?}: Amazon S3 must hold the best candidate's model artifact at \
             {artifact}"
        );

        // What the analysis resolved: the objective it ranked by, the problem type it solved
        // and the candidate cap it ran under.
        assert_eq!(
            pointer_str(&settled, "/ResolvedAttributes/AutoMLJobObjective/MetricName"),
            row.objective_metric,
            "case {case:?}: the resolved objective metric, got {settled}"
        );
        assert_eq!(
            pointer_str(
                &settled,
                "/ResolvedAttributes/AutoMLProblemTypeResolvedAttributes/\
                 TabularResolvedAttributes/ProblemType"
            ),
            row.resolved_problem_type,
            "case {case:?}: the resolved problem type, got {settled}"
        );
        let requested_cap = row
            .tabular
            .pointer("/CompletionCriteria/MaxCandidates")
            .and_then(Value::as_u64);
        assert_eq!(
            settled
                .pointer("/ResolvedAttributes/CompletionCriteria/MaxCandidates")
                .and_then(Value::as_u64),
            Some(requested_cap.unwrap_or(CANDIDATES as u64)),
            "case {case:?}: the resolved candidate cap, got {settled}"
        );

        // Reading the settled job again reports exactly the same thing: the clock moved it,
        // and a read moves nothing.
        let again = settled_read(&emulator, name).await;
        assert_eq!(
            again, settled,
            "case {case:?}: a settled job must read back identically"
        );
    }
}

/// One `DescribeAutoMLJobV2` read of a job already settled.
async fn settled_read(emulator: &Emulator, name: &str) -> Value {
    emulator
        .sagemaker_ok("DescribeAutoMLJobV2", &json!({ "AutoMLJobName": name }))
        .await
}

/// RULE: the metrics a run reports are MEASURED over the caller's own data, so two jobs over
/// different datasets do not report the same objective value.
#[tokio::test]
async fn the_objective_metric_is_measured_over_the_jobs_own_data() {
    let emulator = Emulator::new();
    emulator.given_bucket(BUCKET).await;
    let role_arn = emulator
        .given_role("automl-measure-role", Some(&read_and_write()))
        .await;

    // Two datasets differing in their records, each its own job: a measurement over one
    // cannot be a measurement over the other.
    let datasets = [
        ("narrow", csv_records(30, 2)),
        ("wide", csv_classes(30, 3)),
    ];
    let mut values: Vec<(String, f64)> = Vec::new();
    for (case, data) in datasets {
        let key = format!("measure/{case}.csv");
        emulator.given_object(BUCKET, &key, &data).await;
        let name = format!("automl-measure-{case}");
        let row = Row {
            case: "measurement",
            objects: Vec::new(),
            channels: json!([channel(
                "training",
                &format!("s3://{BUCKET}/{key}"),
                "S3Prefix"
            )]),
            tabular: json!({ "ProblemType": "Regression" }),
            objective: Some(json!({ "MetricName": "MAE" })),
            data_split: None,
            output: format!("s3://{BUCKET}/automl/measure/{case}/"),
            resolved_problem_type: "Regression",
            objective_metric: "MAE",
            objective_direction: "Minimize",
        };
        emulator
            .sagemaker_ok("CreateAutoMLJobV2", &create_body(&name, &role_arn, &row))
            .await;
        let settled = settled(&emulator, case, &name).await;
        assert_eq!(
            str_field(&settled, "AutoMLJobStatus"),
            "Completed",
            "case {case:?}: the job must complete, got {settled}"
        );
        let value = settled
            .pointer("/BestCandidate/FinalAutoMLJobObjectiveMetric/Value")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: no measured objective in {settled}"));
        values.push((case.to_string(), value));
    }
    assert!(
        (values[0].1 - values[1].1).abs() > f64::EPSILON,
        "the objective must be measured over each job's own data, but {:?} and {:?} report \
         the same value",
        values[0],
        values[1]
    );
}
