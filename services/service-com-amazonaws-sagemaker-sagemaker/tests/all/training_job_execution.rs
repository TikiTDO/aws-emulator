// SPDX-License-Identifier: CC0-1.0

//! What a training job's RUN is: the records it reads out of its channels, the model it
//! fits on them, the artifact it writes under `OutputDataConfig.S3OutputPath` and the
//! metrics it reports — plus the execution role's own grants, which decide what the run may
//! read and write.
//!
//! Driven end to end with the emulator's own Amazon S3 and IAM mounted beside SageMaker on
//! ONE shared engine — what `core/router` assembles — so every case reads and writes its
//! objects in the service that OWNS objects and is authorized against the role records IAM
//! itself wrote. Every call travels the real wire through `MultiServiceRouter::dispatch`
//! (`awsJson1_1` with `X-Amz-Target` for SageMaker, path-style requests for Amazon S3,
//! `awsQuery` for IAM), so what is asserted is what an SDK client would see.
//!
//! RULE ONE — an accepted job whose channel data the execution role can read and whose
//! output location it can write RUNS: the clock moves it out of `InProgress` into
//! `Completed`, `DescribeTrainingJob` reports `ModelArtifacts.S3ModelArtifacts` naming an
//! object Amazon S3 actually holds under the declared output path, the metrics the run
//! measured over each channel, and the instants and seconds its work spanned — with
//! `FailureReason` absent.
//!
//! RULE TWO — the artifact is COMPUTED from the caller's data: the model it holds carries
//! the records the run read, the weights it fitted and the error it measured, so two jobs
//! over different data do not produce the same artifact.
//!
//! RULE THREE — the job reads and writes UNDER the execution role: an input data source
//! whose location answers no object that role may list is refused SYNCHRONOUSLY, with the 400
//! `ValidationException` naming the S3 URL, the Region, the role and the bucket; and a run
//! whose own legs fail — the object it cannot read, the artifact it cannot write — lands in
//! the modelled `Failed` status with the `ClientError:` reason naming that leg, never an
//! artifact or a metric for work that produced nothing.
//!
//! RULE FOUR — whether an accepted job RUNS turns on whether the role's trust document
//! admits the SageMaker service principal for `sts:AssumeRole`, not on the FORM that grant
//! is written in: a role admitted by any of those forms backs a job that reads `InProgress`
//! with no `FailureReason` and then settles `Completed` with none either, while a document
//! that withholds the grant is refused with the role-assumption wording and leaves the name
//! absent.
//!
//! The tables span what the rules generalise over: how the channel's location is addressed
//! (one object, a prefix holding several, a manifest naming them), how many records and
//! features the data carries, whether the output path carries a trailing separator, how many
//! channels the job declares, and which Amazon S3 actions the role's policies allow.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call is signed for and every ARN below is scoped to.
const REGION: &str = "us-east-1";

/// The account id the emulator mints its ARNs under.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Long enough to pass `state::TRAINING_JOB_ENDS_AFTER`, so the clock owes the job its
/// terminal status. Read from the constant itself, so the wait tracks the span the state
/// gives a job's work.
const PAST_ENDS_DUE: Duration =
    service_com_amazonaws_sagemaker_sagemaker::state::TRAINING_JOB_ENDS_AFTER
        .saturating_mul(2)
        .saturating_add(Duration::from_millis(100));

/// The bucket every case stages its channel objects in and writes its artifact to.
pub const BUCKET: &str = "sagemaker-training-run";

/// The service principal a SageMaker execution role's trust policy admits.
const SERVICE_PRINCIPAL: &str = service_com_amazonaws_sagemaker_sagemaker::state::SERVICE_PRINCIPAL;

/// A SigV4 credential scope naming `service`: it is what routes a request to that service.
/// The key id and signature are not credentials — this harness turns request authentication
/// off, so nothing is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// SageMaker, Amazon S3 and IAM on ONE shared engine, the way the router mounts every
/// service.
pub struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One SageMaker call, as a client makes it.
    pub async fn sagemaker(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header("authorization", authorization("sagemaker"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One SageMaker call that must be served.
    pub async fn sagemaker_ok(&self, target: &str, body: &Value) -> Value {
        let (status, answered) = self.sagemaker(target, body).await;
        assert_eq!(
            status, 200,
            "case {target} {body}: expected HTTP 200, got {status} with {answered}"
        );
        answered
    }

    /// One raw path-style request to the emulator's own Amazon S3 front door: the bucket and
    /// the key live in the URI and the body is the object's own bytes.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().to_vec())
    }

    /// `PUT /<bucket>` — `CreateBucket`.
    pub async fn given_bucket(&self, bucket: &str) {
        let (status, body) = self.s3("PUT", &format!("/{bucket}"), &[]).await;
        assert_eq!(
            status,
            200,
            "the bucket {bucket} must be created, got {status} {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// `PUT /<bucket>/<key>` — `PutObject`, storing `body` as the object's own bytes.
    pub async fn given_object(&self, bucket: &str, key: &str, body: &str) {
        let (status, answered) = self
            .s3("PUT", &format!("/{bucket}/{key}"), body.as_bytes())
            .await;
        assert_eq!(
            status,
            200,
            "the object {key} must be stored, got {status} {}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// `GET /<bucket>/<key>` — `GetObject` — as the bytes it holds, or `None` when the store
    /// does not hold it.
    pub async fn object_bytes(&self, bucket: &str, key: &str) -> Option<Vec<u8>> {
        let (status, body) = self.s3("GET", &format!("/{bucket}/{key}"), &[]).await;
        (status == 200).then_some(body)
    }

    /// One IAM call (`awsQuery`), as a client makes it.
    async fn iam(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization("iam"))
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create `role_name` through IAM's own `CreateRole`, trusting the SageMaker service
    /// principal, and attach `permissions` as an inline policy when the case gives one.
    /// Answers the role's ARN.
    pub async fn given_role(&self, role_name: &str, permissions: Option<&str>) -> String {
        self.given_role_trusting(role_name, &sagemaker_trust(), permissions)
            .await
    }

    /// The same, for a role whose trust document the case states itself: one creation path,
    /// so a case exercising another FORM of the same grant is stored exactly like the
    /// canonical one.
    async fn given_role_trusting(
        &self,
        role_name: &str,
        trust: &str,
        permissions: Option<&str>,
    ) -> String {
        let (status, body) = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", role_name),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "the role {role_name} must be created, got {status} {body}"
        );
        if let Some(document) = permissions {
            let (status, body) = self
                .iam(
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name),
                        ("PolicyName", "training-data-access"),
                        ("PolicyDocument", document),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "the policy of {role_name} must be stored, got {status} {body}"
            );
        }
        format!("arn:aws:iam::{ACCOUNT}:role/{role_name}")
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component with the maintained
/// RFC 3986 encoder, leaving only the unreserved characters literal.
fn form_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The trust document a SageMaker execution role carries: `sts:AssumeRole` for the service
/// principal, which is what `CreateTrainingJob` resolves before it accepts a job.
fn sagemaker_trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": SERVICE_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// An identity policy allowing `actions` on `resource`.
fn allow(actions: &[&str], resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resource }],
    })
    .to_string()
}

/// The policy a role that may list the training data, read it and write the artifact
/// carries: the object actions on the objects of the bucket, and the listing on the BUCKET
/// itself, which is the resource `s3:ListBucket` is granted on.
pub fn read_and_write() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": ["s3:GetObject", "s3:PutObject"],
                "Resource": format!("arn:aws:s3:::{BUCKET}/*"),
            },
            {
                "Effect": "Allow",
                "Action": "s3:ListBucket",
                "Resource": format!("arn:aws:s3:::{BUCKET}"),
            },
        ],
    })
    .to_string()
}

/// A `CreateTrainingJob` body: `channels` as its `InputDataConfig` and `output` as its
/// output location.
pub fn training_body(name: &str, role_arn: &str, channels: Value, output: &str) -> Value {
    json!({
        "TrainingJobName": name,
        "AlgorithmSpecification": {
            "TrainingImage": format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/trainer:1"),
            "TrainingInputMode": "File",
        },
        "RoleArn": role_arn,
        "InputDataConfig": channels,
        "OutputDataConfig": { "S3OutputPath": output },
        "ResourceConfig": {
            "InstanceType": "ml.m5.large",
            "InstanceCount": 1,
            "VolumeSizeInGB": 10,
        },
        "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
    })
}

/// One channel of an `InputDataConfig`, addressing `uri` as `data_type`.
pub fn channel(name: &str, uri: &str, data_type: &str) -> Value {
    json!({
        "ChannelName": name,
        "DataSource": { "S3DataSource": { "S3DataType": data_type, "S3Uri": uri } },
        "ContentType": "text/csv",
    })
}

/// A top-level string member of a response.
pub fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Comma-separated records whose first column is the target and whose remaining columns are
/// the features, as a training channel carries them: `rows` records of `features` features,
/// with the target a linear function of them.
pub fn csv_records(rows: usize, features: usize) -> String {
    (0..rows)
        .map(|row| {
            let values: Vec<f64> = (0..features)
                .map(|column| (row + column + 1) as f64)
                .collect();
            let target: f64 = 3.0 + values.iter().sum::<f64>();
            let mut record = vec![format!("{target}")];
            record.extend(values.iter().map(|value| format!("{value}")));
            format!("{}\n", record.join(","))
        })
        .collect()
}

/// The URI `ModelArtifacts.S3ModelArtifacts` must name for `job` under `output`.
pub fn artifact_uri(output: &str, job: &str) -> String {
    format!("{}/{job}/output/model.tar.gz", output.trim_end_matches('/'))
}

/// The key of that same object inside [`BUCKET`].
pub fn artifact_key(output: &str, job: &str) -> String {
    artifact_uri(output, job)
        .strip_prefix(&format!("s3://{BUCKET}/"))
        .expect("the output location is in the harness bucket")
        .to_string()
}

/// The model document the archive holds: the gzip-compressed tar is decompressed and its
/// single `model.json` entry parsed, so what is asserted is what a client reading the
/// artifact out of Amazon S3 would read.
fn model_document(archive: &[u8]) -> Value {
    use std::io::Read;

    let decoder = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(decoder);
    for entry in tar.entries().expect("the archive lists its entries") {
        let mut entry = entry.expect("an archive entry");
        let path = entry
            .path()
            .expect("an entry path")
            .to_string_lossy()
            .to_string();
        if path != service_com_amazonaws_sagemaker_sagemaker::state::MODEL_ARCHIVE_ENTRY {
            continue;
        }
        let mut body = String::new();
        entry.read_to_string(&mut body).expect("the entry's bytes");
        return serde_json::from_str(&body).expect("the model document is JSON");
    }
    panic!("the archive holds no model entry: {} bytes", archive.len());
}

/// RULE ONE and RULE TWO: an accepted job whose data the execution role can read RUNS, and
/// what it reports is what it produced.
///
/// The rows differ along how the channel's location is addressed, how many records and
/// features it holds, how many channels the job declares, and whether the output path
/// carries a trailing separator.
#[tokio::test]
async fn an_accepted_training_job_runs_and_reports_the_artifact_it_wrote() {
    /// One case: the job's name, its channels, the output location, and the records each
    /// channel was staged with.
    struct Row {
        case: &'static str,
        job: &'static str,
        channels: Value,
        output: String,
        /// Channel name -> how many records the run must have read for it.
        records: Vec<(&'static str, usize)>,
        /// How many features the fitted model must carry.
        features: usize,
    }

    let emulator = Emulator::new();
    emulator.given_bucket(BUCKET).await;
    let role = emulator
        .given_role("sagemaker-run-role", Some(&read_and_write()))
        .await;

    // One object addressed by its own key.
    emulator
        .given_object(BUCKET, "one/data.csv", &csv_records(6, 2))
        .await;
    // Two objects under a prefix.
    emulator
        .given_object(BUCKET, "many/part-1.csv", &csv_records(4, 1))
        .await;
    emulator
        .given_object(BUCKET, "many/part-2.csv", &csv_records(5, 1))
        .await;
    // A manifest naming one object by a prefix and a suffix.
    emulator
        .given_object(BUCKET, "listed/rows.csv", &csv_records(7, 3))
        .await;
    emulator
        .given_object(
            BUCKET,
            "manifest.json",
            &json!([{ "prefix": format!("s3://{BUCKET}/listed/") }, "rows.csv"]).to_string(),
        )
        .await;
    // A training channel beside a validation channel.
    emulator
        .given_object(BUCKET, "split/train.csv", &csv_records(8, 2))
        .await;
    emulator
        .given_object(BUCKET, "split/validation.csv", &csv_records(3, 2))
        .await;

    let rows = vec![
        Row {
            case: "one object addressed by its own key",
            job: "run-one-object",
            channels: json!([channel(
                "train",
                &format!("s3://{BUCKET}/one/data.csv"),
                "S3Prefix"
            )]),
            output: format!("s3://{BUCKET}/output/"),
            records: vec![("train", 6)],
            features: 2,
        },
        Row {
            case: "a prefix holding two objects, and no trailing separator on the output",
            job: "run-many-objects",
            channels: json!([channel("train", &format!("s3://{BUCKET}/many/"), "S3Prefix")]),
            output: format!("s3://{BUCKET}/nested/output"),
            records: vec![("train", 9)],
            features: 1,
        },
        Row {
            case: "a manifest naming the objects",
            job: "run-manifest",
            channels: json!([channel(
                "train",
                &format!("s3://{BUCKET}/manifest.json"),
                "ManifestFile"
            )]),
            output: format!("s3://{BUCKET}/output/"),
            records: vec![("train", 7)],
            features: 3,
        },
        Row {
            case: "a training channel beside a validation channel",
            job: "run-two-channels",
            channels: json!([
                channel("train", &format!("s3://{BUCKET}/split/train.csv"), "S3Prefix"),
                channel(
                    "validation",
                    &format!("s3://{BUCKET}/split/validation.csv"),
                    "S3Prefix"
                ),
            ]),
            output: format!("s3://{BUCKET}/output/"),
            records: vec![("train", 8), ("validation", 3)],
            features: 2,
        },
    ];

    for row in rows {
        let body = training_body(row.job, &role, row.channels.clone(), &row.output);
        let created = emulator.sagemaker_ok("CreateTrainingJob", &body).await;
        assert_eq!(
            str_field(&created, "TrainingJobArn"),
            format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:training-job/{}", row.job),
            "case {:?}: the minted ARN, got {created}",
            row.case
        );

        // The move out of `InProgress` is the clock's, so a read straight after the create
        // still observes the accepted job.
        let accepted = emulator
            .sagemaker_ok(
                "DescribeTrainingJob",
                &json!({ "TrainingJobName": row.job }),
            )
            .await;
        assert_eq!(
            str_field(&accepted, "TrainingJobStatus"),
            "InProgress",
            "case {:?}: an accepted job is in progress, got {accepted}",
            row.case
        );

        tokio::time::sleep(PAST_ENDS_DUE).await;
        let settled = emulator
            .sagemaker_ok(
                "DescribeTrainingJob",
                &json!({ "TrainingJobName": row.job }),
            )
            .await;
        assert_eq!(
            (
                str_field(&settled, "TrainingJobStatus"),
                str_field(&settled, "SecondaryStatus"),
                settled.get("FailureReason").cloned(),
            ),
            (
                "Completed".to_string(),
                "Completed".to_string(),
                None,
            ),
            "case {:?}: a run that finished reports no failure, got {settled}",
            row.case
        );
        let reported = settled
            .pointer("/ModelArtifacts/S3ModelArtifacts")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported,
            artifact_uri(&row.output, row.job),
            "case {:?}: the artifact URI under the declared output path, got {settled}",
            row.case
        );
        // The object the describe names is an object Amazon S3 itself holds.
        let archive = emulator
            .object_bytes(BUCKET, &artifact_key(&row.output, row.job))
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: Amazon S3 must hold the artifact {reported}",
                    row.case
                )
            });
        let document = model_document(&archive);
        let weights = document
            .pointer("/model/weights")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            weights.len(),
            row.features,
            "case {:?}: the fitted model must carry one weight per feature, got {document}",
            row.case
        );
        assert!(
            document
                .pointer("/model/intercept")
                .and_then(Value::as_f64)
                .is_some()
                && weights.iter().all(|weight| weight.as_f64().is_some()),
            "case {:?}: every fitted parameter must be a number, got {document}",
            row.case
        );
        // What the run READ, per channel, is what the staged objects hold.
        let read: Vec<(String, u64)> = document
            .get("channels")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| {
                (
                    str_field(entry, "name"),
                    entry.get("records").and_then(Value::as_u64).unwrap_or(0),
                )
            })
            .collect();
        let expected: Vec<(String, u64)> = row
            .records
            .iter()
            .map(|(name, count)| ((*name).to_string(), *count as u64))
            .collect();
        assert_eq!(
            read, expected,
            "case {:?}: the records the run read, got {document}",
            row.case
        );

        // One metric per channel it measured, named for that channel.
        let metrics: Vec<String> = settled
            .get("FinalMetricDataList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|metric| str_field(metric, "MetricName"))
            .collect();
        let named: Vec<String> = row
            .records
            .iter()
            .map(|(name, _)| format!("{name}:rmse"))
            .collect();
        assert_eq!(
            metrics, named,
            "case {:?}: one measured metric per channel, got {settled}",
            row.case
        );
        assert!(
            settled
                .get("FinalMetricDataList")
                .and_then(Value::as_array)
                .is_some_and(|list| list
                    .iter()
                    .all(|metric| metric.get("Value").and_then(Value::as_f64).is_some())),
            "case {:?}: every metric must carry the value it measured, got {settled}",
            row.case
        );

        // The run's own span: it started once the job was accepted and was over by the
        // instant the clock settled it, so the whole seconds between them are reported.
        for member in [
            "TrainingStartTime",
            "TrainingEndTime",
            "TrainingTimeInSeconds",
            "BillableTimeInSeconds",
        ] {
            assert!(
                settled.get(member).is_some(),
                "case {:?}: {member} must report the run's own span, got {settled}",
                row.case
            );
        }

        // The terminal status is the clock's, so a later read answers the same thing.
        let reread = emulator
            .sagemaker_ok(
                "DescribeTrainingJob",
                &json!({ "TrainingJobName": row.job }),
            )
            .await;
        assert_eq!(
            reread, settled,
            "case {:?}: a settled job must read back identically, got {reread}",
            row.case
        );
    }
}

/// RULE TWO: the artifact is computed from the data, so two jobs over different records do
/// not produce the same model.
#[tokio::test]
async fn the_fitted_model_follows_the_records_the_run_read() {
    let emulator = Emulator::new();
    emulator.given_bucket(BUCKET).await;
    let role = emulator
        .given_role("sagemaker-fit-role", Some(&read_and_write()))
        .await;

    // Two channels whose records carry DIFFERENT linear relations: y = x + 1 and y = 4x.
    emulator
        .given_object(BUCKET, "fit/gentle.csv", "2,1\n3,2\n4,3\n5,4\n")
        .await;
    emulator
        .given_object(BUCKET, "fit/steep.csv", "4,1\n8,2\n12,3\n16,4\n")
        .await;

    let mut fitted = Vec::new();
    for (case, key, slope) in [
        ("y = x + 1", "fit/gentle.csv", 1.0_f64),
        ("y = 4x", "fit/steep.csv", 4.0_f64),
    ] {
        let job = format!("fit-{}", key.replace(['/', '.'], "-"));
        let output = format!("s3://{BUCKET}/fit-output/");
        emulator
            .sagemaker_ok(
                "CreateTrainingJob",
                &training_body(
                    &job,
                    &role,
                    json!([channel("train", &format!("s3://{BUCKET}/{key}"), "S3Prefix")]),
                    &output,
                ),
            )
            .await;
        tokio::time::sleep(PAST_ENDS_DUE).await;
        let settled = emulator
            .sagemaker_ok("DescribeTrainingJob", &json!({ "TrainingJobName": job }))
            .await;
        assert_eq!(
            str_field(&settled, "TrainingJobStatus"),
            "Completed",
            "case {case:?}: the run must finish, got {settled}"
        );
        let archive = emulator
            .object_bytes(BUCKET, &artifact_key(&output, &job))
            .await
            .unwrap_or_else(|| panic!("case {case:?}: Amazon S3 must hold the artifact"));
        let document = model_document(&archive);
        let weight = document
            .pointer("/model/weights/0")
            .and_then(Value::as_f64)
            .unwrap_or_default();
        // The fit MEASURES the relation: a dataset whose target rises four times as fast
        // carries the larger weight, and the error it reports is a fraction of the data's
        // own spread.
        let error = settled
            .pointer("/FinalMetricDataList/0/Value")
            .and_then(Value::as_f64)
            .unwrap_or(f64::MAX);
        assert!(
            error < slope,
            "case {case:?}: the measured error {error} must be well under the relation's own \
             slope {slope}, got {settled}"
        );
        fitted.push((case, weight));
    }
    let gentle = fitted[0].1;
    let steep = fitted[1].1;
    assert!(
        steep > gentle,
        "case the two fits: the steeper data must carry the larger weight, got {gentle} \
         against {steep}"
    );
}

/// RULE THREE: the job reads its channels UNDER the execution role, so an input data source
/// whose location answers no object that role may list is refused SYNCHRONOUSLY with a 400
/// `ValidationException` naming the S3 URL, the Region, the role and the bucket — no job is
/// stored and no artifact is written.
///
/// The rows differ along which actions the role's policies allow and whether the location
/// holds an object at all.
#[tokio::test]
async fn an_input_data_source_that_answers_no_object_is_refused_synchronously() {
    /// One case: what the role is allowed, which location the channel names, and whether the
    /// create is refused.
    struct Row {
        case: &'static str,
        role: &'static str,
        policy: Option<String>,
        key: &'static str,
        refused: bool,
    }

    let emulator = Emulator::new();
    emulator.given_bucket(BUCKET).await;
    emulator
        .given_object(BUCKET, "grants/data.csv", &csv_records(5, 2))
        .await;

    let rows = vec![
        Row {
            case: "every action the job makes",
            role: "grants-all",
            policy: Some(read_and_write()),
            key: "grants/data.csv",
            refused: false,
        },
        Row {
            case: "a role that may read and write but not list the bucket",
            role: "grants-no-listing",
            policy: Some(allow(
                &["s3:GetObject", "s3:PutObject"],
                format!("arn:aws:s3:::{BUCKET}/*").as_str(),
            )),
            key: "grants/data.csv",
            refused: true,
        },
        Row {
            case: "no permissions policy at all",
            role: "grants-none",
            policy: None,
            key: "grants/data.csv",
            refused: true,
        },
        Row {
            case: "a prefix the bucket holds no object under",
            role: "grants-empty-prefix",
            policy: Some(read_and_write()),
            key: "absent/",
            refused: true,
        },
    ];

    for row in rows {
        let role = emulator.given_role(row.role, row.policy.as_deref()).await;
        let job = format!("grant-case-{}", row.role);
        let output = format!("s3://{BUCKET}/grants-output/{}/", row.role);
        let input_uri = format!("s3://{BUCKET}/{}", row.key);
        let (status, answered) = emulator
            .sagemaker(
                "CreateTrainingJob",
                &training_body(
                    &job,
                    &role,
                    json!([channel("train", &input_uri, "S3Prefix")]),
                    &output,
                ),
            )
            .await;

        if !row.refused {
            assert_eq!(
                status, 200,
                "case {:?}: a data source that answers an object is accepted, got {status} \
                 with {answered}",
                row.case
            );
            tokio::time::sleep(PAST_ENDS_DUE).await;
            let settled = emulator
                .sagemaker_ok("DescribeTrainingJob", &json!({ "TrainingJobName": job }))
                .await;
            assert_eq!(
                str_field(&settled, "TrainingJobStatus"),
                "Completed",
                "case {:?}: the run must finish, got {settled}",
                row.case
            );
            assert!(
                emulator
                    .object_bytes(BUCKET, &artifact_key(&output, &job))
                    .await
                    .is_some(),
                "case {:?}: the artifact must be written under {output}",
                row.case
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {:?}: expected a 400 ValidationException, got {status} with {answered}",
            row.case
        );
        // The refusal carries the four values the caller has to act on: the URL it gave, the
        // Region it addressed, the role it supplied and the bucket that role must be allowed
        // to list.
        let message = str_field(&answered, "message");
        for named in [input_uri.as_str(), REGION, role.as_str(), BUCKET] {
            assert!(
                message.contains(named),
                "case {:?}: the refusal must name {named:?}, got {message:?}",
                row.case
            );
        }
        // Nothing was stored, and nothing was written under the output path.
        let (status, described) = emulator
            .sagemaker("DescribeTrainingJob", &json!({ "TrainingJobName": job }))
            .await;
        assert_eq!(
            (status, error_code(&described).as_str()),
            (400, "ValidationException"),
            "case {:?}: a refused create stores no job, got {status} with {described}",
            row.case
        );
        assert!(
            emulator
                .object_bytes(BUCKET, &artifact_key(&output, &job))
                .await
                .is_none(),
            "case {:?}: a refused create writes no artifact under {output}",
            row.case
        );
    }
}

/// RULE: there is no `Completed` for a run that produced nothing — a job accepted with a data
/// source the run then cannot read, or an artifact it cannot write, lands in the modelled
/// `Failed` status with the `ClientError:` reason naming the leg that failed, and no artifact
/// is written.
///
/// The rows differ along which leg fails: the object read and the artifact write.
#[tokio::test]
async fn a_run_that_produced_nothing_reports_the_condition_that_stopped_it() {
    let emulator = Emulator::new();
    emulator.given_bucket(BUCKET).await;
    emulator
        .given_object(BUCKET, "legs/data.csv", &csv_records(5, 2))
        .await;
    let bucket_arn = format!("arn:aws:s3:::{BUCKET}");
    let objects_arn = format!("arn:aws:s3:::{BUCKET}/*");

    // Each row: the case, the role's name, what it is allowed, the object the channel names,
    // and the reason prefix the failed job must carry.
    let rows: Vec<(&str, &str, String, &str, &str)> = vec![
        (
            "the object cannot be read",
            "legs-no-read",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": "s3:ListBucket", "Resource": bucket_arn },
                    { "Effect": "Allow", "Action": "s3:PutObject", "Resource": objects_arn },
                ],
            })
            .to_string(),
            "legs/data.csv",
            "ClientError: Data download failed:",
        ),
        (
            "the artifact cannot be written",
            "legs-no-write",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": "s3:ListBucket", "Resource": bucket_arn },
                    { "Effect": "Allow", "Action": "s3:GetObject", "Resource": objects_arn },
                ],
            })
            .to_string(),
            "legs/data.csv",
            "ClientError: Model artifact upload failed:",
        ),
    ];

    for (case, role_name, policy, key, prefix) in rows {
        let role = emulator.given_role(role_name, Some(&policy)).await;
        let job = format!("leg-case-{role_name}");
        let output = format!("s3://{BUCKET}/legs-output/{role_name}/");
        emulator
            .sagemaker_ok(
                "CreateTrainingJob",
                &training_body(
                    &job,
                    &role,
                    json!([channel("train", &format!("s3://{BUCKET}/{key}"), "S3Prefix")]),
                    &output,
                ),
            )
            .await;
        tokio::time::sleep(PAST_ENDS_DUE).await;
        let settled = emulator
            .sagemaker_ok("DescribeTrainingJob", &json!({ "TrainingJobName": job }))
            .await;
        assert_eq!(
            (
                str_field(&settled, "TrainingJobStatus"),
                str_field(&settled, "SecondaryStatus"),
            ),
            ("Failed".to_string(), "Failed".to_string()),
            "case {case:?}: a run that produced nothing must fail, got {settled}"
        );
        let reason = str_field(&settled, "FailureReason");
        assert!(
            reason.starts_with(prefix),
            "case {case:?}: the reason must start with {prefix:?}, got {reason:?}"
        );
        assert!(
            settled.get("FinalMetricDataList").is_none()
                && emulator
                    .object_bytes(BUCKET, &artifact_key(&output, &job))
                    .await
                    .is_none(),
            "case {case:?}: a failed run reports no metric and writes no artifact, got \
             {settled}"
        );
    }
}

/// RULE: a role whose trust policy does not admit the SageMaker service principal backs no
/// training run at all, so the create is refused synchronously with a 400
/// `ValidationException` naming the role the request supplied, and no job is stored.
///
/// The rows differ along which principal the trust document admits and which action it
/// allows.
#[tokio::test]
async fn a_role_sagemaker_cannot_assume_is_refused_before_any_job_is_stored() {
    let emulator = Emulator::new();
    emulator.given_bucket(BUCKET).await;
    emulator
        .given_object(BUCKET, "trust/data.csv", &csv_records(4, 1))
        .await;

    // Rows differ along the trust document: another service principal, the wrong action,
    // and the admitted principal with the documented action.
    let rows: Vec<(&str, &str, Value, bool)> = vec![
        (
            "another service principal",
            "trust-ec2",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "ec2.amazonaws.com" },
                    "Action": "sts:AssumeRole",
                }],
            }),
            false,
        ),
        (
            "the service principal with another action",
            "trust-wrong-action",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": "sts:TagSession",
                }],
            }),
            false,
        ),
        (
            "the service principal with the documented action",
            "trust-sagemaker",
            json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": "sts:AssumeRole",
                }],
            }),
            true,
        ),
    ];

    for (case, role_name, trust, accepted) in rows {
        let (status, body) = emulator
            .iam(
                "CreateRole",
                &[
                    ("RoleName", role_name),
                    ("AssumeRolePolicyDocument", &trust.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the role must be created, got {status} {body}"
        );
        let (status, body) = emulator
            .iam(
                "PutRolePolicy",
                &[
                    ("RoleName", role_name),
                    ("PolicyName", "training-data-access"),
                    ("PolicyDocument", &read_and_write()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the policy must be stored, got {status} {body}"
        );

        let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{role_name}");
        let job = format!("trust-case-{role_name}");
        let (status, answered) = emulator
            .sagemaker(
                "CreateTrainingJob",
                &training_body(
                    &job,
                    &role_arn,
                    json!([channel(
                        "train",
                        &format!("s3://{BUCKET}/trust/data.csv"),
                        "S3Prefix"
                    )]),
                    &format!("s3://{BUCKET}/trust-output/"),
                ),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a role SageMaker can assume is accepted, got {status} with \
                 {answered}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} with {answered}"
        );
        assert!(
            str_field(&answered, "message").contains(&role_arn),
            "case {case:?}: the refusal must name the role the request supplied, got \
             {answered}"
        );
        // Nothing was stored: the job's own reader says so.
        let (status, described) = emulator
            .sagemaker("DescribeTrainingJob", &json!({ "TrainingJobName": job }))
            .await;
        assert_eq!(
            (status, error_code(&described).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a refused create stores no job, got {status} with {described}"
        );
    }
}

/// How SageMaker words a `RoleArn` it cannot assume. The fixed text is everything but the
/// role ARN, which is the one the REQUEST supplied.
fn could_not_assume_role(role_arn: &str) -> String {
    format!(
        "Could not assume role {role_arn}. Please ensure that the role exists and allows \
         principal '{SERVICE_PRINCIPAL}' to assume the role."
    )
}

/// How SageMaker words a training job this account holds no record of.
const RESOURCE_NOT_FOUND: &str = "Requested resource not found.";

/// RULE FOUR: whether an accepted job RUNS turns on whether the execution role admits the
/// SageMaker service principal for the assume — not on the FORM the trust document states
/// that grant in. So a role admitted by any of those forms backs a job that reports
/// `InProgress` with no `FailureReason` on the first read and then the terminal `Completed`
/// with none either, naming an artifact Amazon S3 holds; a role whose document withholds the
/// grant is refused with the 400 `ValidationException` whose message names the role as sent,
/// and the name it refused stays absent.
///
/// The rows differ along what the rule generalises over: whether `Principal.Service` names
/// one principal or a list of them, whether the grant sits in one statement or among several,
/// whether `Statement` is an object or a list, whether `Action` names the assume itself, a
/// list containing it or a wildcard covering it, and — for the withholding half — whether the
/// document names another principal, another action, or denies the grant outright.
#[tokio::test]
async fn an_admitted_execution_role_runs_the_job_whatever_form_its_trust_document_takes() {
    /// One case: the role the trust document is attached to, the document itself, and
    /// whether SageMaker can assume that role.
    struct Row {
        case: &'static str,
        role: &'static str,
        trust: Value,
        admitted: bool,
    }

    let emulator = Emulator::new();
    emulator.given_bucket(BUCKET).await;
    emulator
        .given_object(BUCKET, "control/data.csv", &csv_records(6, 2))
        .await;
    let output = format!("s3://{BUCKET}/control-output/");

    let rows = vec![
        Row {
            case: "the service principal and the assume, each as one string",
            role: "control-one-string",
            trust: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": "sts:AssumeRole",
                }],
            }),
            admitted: true,
        },
        Row {
            case: "the service principal among a list of principals",
            role: "control-principal-list",
            trust: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": ["ec2.amazonaws.com", SERVICE_PRINCIPAL] },
                    "Action": "sts:AssumeRole",
                }],
            }),
            admitted: true,
        },
        Row {
            case: "the grant in the second of two statements",
            role: "control-second-statement",
            trust: json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": "ec2.amazonaws.com" },
                        "Action": "sts:AssumeRole",
                    },
                    {
                        "Effect": "Allow",
                        "Principal": { "Service": SERVICE_PRINCIPAL },
                        "Action": "sts:AssumeRole",
                    },
                ],
            }),
            admitted: true,
        },
        Row {
            case: "one statement as an object rather than a list, and the assume among a \
                   list of actions",
            role: "control-statement-object",
            trust: json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": ["sts:TagSession", "sts:AssumeRole"],
                },
            }),
            admitted: true,
        },
        Row {
            case: "a prefix wildcard covering the assume",
            role: "control-wildcard-action",
            trust: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": "sts:Assume*",
                }],
            }),
            admitted: true,
        },
        Row {
            case: "another service principal only",
            role: "control-other-principal",
            trust: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "ec2.amazonaws.com" },
                    "Action": "sts:AssumeRole",
                }],
            }),
            admitted: false,
        },
        Row {
            case: "the service principal, for another action",
            role: "control-other-action",
            trust: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": "s3:GetObject",
                }],
            }),
            admitted: false,
        },
        Row {
            case: "the service principal, explicitly denied the assume",
            role: "control-denied",
            trust: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": { "Service": SERVICE_PRINCIPAL },
                    "Action": "sts:AssumeRole",
                }],
            }),
            admitted: false,
        },
    ];

    for row in rows {
        let role_arn = emulator
            .given_role_trusting(row.role, &row.trust.to_string(), Some(&read_and_write()))
            .await;
        let job = format!("control-job-{}", row.role);
        let (status, answered) = emulator
            .sagemaker(
                "CreateTrainingJob",
                &training_body(
                    &job,
                    &role_arn,
                    json!([channel(
                        "train",
                        &format!("s3://{BUCKET}/control/data.csv"),
                        "S3Prefix"
                    )]),
                    &output,
                ),
            )
            .await;

        if !row.admitted {
            assert_eq!(
                (
                    status,
                    error_code(&answered),
                    str_field(&answered, "message")
                ),
                (
                    400,
                    "ValidationException".to_string(),
                    could_not_assume_role(&role_arn)
                ),
                "case {:?}: the role refusal as SageMaker words it, got {status} with \
                 {answered}",
                row.case
            );
            let (status, described) = emulator
                .sagemaker("DescribeTrainingJob", &json!({ "TrainingJobName": &job }))
                .await;
            assert_eq!(
                (
                    status,
                    error_code(&described),
                    str_field(&described, "message")
                ),
                (
                    400,
                    "ValidationException".to_string(),
                    RESOURCE_NOT_FOUND.to_string()
                ),
                "case {:?}: a refused create leaves the name absent, got {status} with \
                 {described}",
                row.case
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {:?}: a role SageMaker can assume is accepted, got {status} with {answered}",
            row.case
        );
        // The first read a waiting client makes: the job is RUNNING, and a running job has
        // no failure to report.
        let accepted = emulator
            .sagemaker_ok("DescribeTrainingJob", &json!({ "TrainingJobName": &job }))
            .await;
        assert_eq!(
            (
                str_field(&accepted, "TrainingJobStatus"),
                accepted.get("FailureReason").cloned()
            ),
            ("InProgress".to_string(), None),
            "case {:?}: an accepted job is in progress and has not failed, got {accepted}",
            row.case
        );

        tokio::time::sleep(PAST_ENDS_DUE).await;
        let settled = emulator
            .sagemaker_ok("DescribeTrainingJob", &json!({ "TrainingJobName": &job }))
            .await;
        assert_eq!(
            (
                str_field(&settled, "TrainingJobStatus"),
                settled.get("FailureReason").cloned()
            ),
            ("Completed".to_string(), None),
            "case {:?}: the job must settle into the outcome of its own run, got {settled}",
            row.case
        );
        let reported = settled
            .pointer("/ModelArtifacts/S3ModelArtifacts")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported,
            artifact_uri(&output, &job),
            "case {:?}: the artifact URI under the declared output path, got {settled}",
            row.case
        );
        assert!(
            emulator
                .object_bytes(BUCKET, &artifact_key(&output, &job))
                .await
                .is_some(),
            "case {:?}: Amazon S3 must hold the artifact {reported}",
            row.case
        );
    }
}
