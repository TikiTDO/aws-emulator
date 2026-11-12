// SPDX-License-Identifier: CC0-1.0

//! An endpoint SERVES the model deployed behind it: a client that trains a model, deploys the
//! artifact and invokes the endpoint is answered what that model computes for its payload.
//!
//! The rules under test:
//!
//! * RULE ONE — `InvokeEndpoint` on an `InService` endpoint answers the prediction the model
//!   behind the variant it routes to computes for the payload: one prediction per record, in
//!   the media type the request's `Accept` asks for, with
//!   `x-Amzn-Invoked-Production-Variant` naming the variant that served it. The answer is
//!   COMPUTED, so the same payload is answered the same way twice and two different payloads
//!   are answered differently.
//! * RULE TWO — where there is nothing to compute from, nothing is synthesised: a variant
//!   whose model names no model artifact, and a payload the model cannot be applied to, are
//!   both answered with the operation's modelled 424 `ModelError` naming the condition.
//!
//! Only a cross-service suite can ask this: the flow spans four services over ONE engine
//! exactly as `core/router/src/lib.rs` mounts them — IAM for the role the training run reads
//! and writes under, Amazon S3 for the training data and the model artifact, the SageMaker
//! control plane to train and deploy, and the SageMaker runtime to invoke. Neither crate's
//! own test target can see both sides of it.
//!
//! The rows differ along what RULE ONE generalises over: the media type and the shape of the
//! payload (comma-separated records, a JSON list of records, a JSON `instances` list), how
//! many records one invocation carries (one, several), how many feature columns the model was
//! fitted on, and the media type the answer is asked for in (none, `text/csv`,
//! `application/json`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::form_encode;

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// The account the emulator serves.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The bucket the training data and the model artifact live in.
const BUCKET: &str = "sagemaker-serving-run";

/// The role the training run reads and writes under.
const ROLE: &str = "sagemaker-serving-role";

/// How long a resource is polled for the status it has to reach before a case fails, and how
/// often. What the poll waits for is the CONDITION, re-read in a loop, so a loaded host does
/// not fail a case the clock would have served.
const SETTLE_BUDGET: Duration = Duration::from_secs(90);
const SETTLE_POLL: Duration = Duration::from_millis(50);

/// A SigV4 `Authorization` header whose credential scope names `signing_name` — the routing
/// key the router claims a request by. Neither the key id nor the signature is a credential:
/// this router runs with request authentication off, so nothing here is verified.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// The four services the flow spans, on ONE engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's concern, not any of these services'.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One HTTP exchange: the status, the response headers and the raw body.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: Vec<u8>,
}

impl Answer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// One response header's value, or the empty string when the answer carried none.
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// The wire error shape the answer names, as a client resolves it.
    fn error_code(&self) -> String {
        let named = match self.headers.get("x-amzn-errortype") {
            Some(value) => value.to_str().unwrap_or_default().to_string(),
            None => self
                .json()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        named.split(':').next().unwrap_or_default().to_string()
    }

    fn message(&self) -> String {
        let rendered = self.json();
        rendered
            .get("message")
            .or_else(|| rendered.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// One request to the service `signing_name` serves.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization(signing_name));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body: body.to_vec(),
    }
}

/// One SageMaker control-plane call (`awsJson1_1`).
async fn control(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let answer = send(
        router,
        "sagemaker",
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &format!("SageMaker.{operation}")),
        ],
        body.to_string().into_bytes(),
    )
    .await;
    (answer.status, answer.json())
}

/// One SageMaker control-plane call that must be served.
async fn control_ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, answered) = control(router, operation, body).await;
    assert_eq!(
        status, 200,
        "setup {operation} {body}: expected HTTP 200, got {status} with {answered}"
    );
    answered
}

/// `POST /endpoints/{endpoint}/invocations`, the one route `InvokeEndpoint` is bound to.
async fn invoke(
    router: &MultiServiceRouter,
    endpoint: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    send(
        router,
        "sagemaker",
        "POST",
        &format!("/endpoints/{endpoint}/invocations"),
        headers,
        body.to_vec(),
    )
    .await
}

/// `PUT /<bucket>` — `CreateBucket`.
async fn given_bucket(router: &MultiServiceRouter) {
    let answer = send(router, "s3", "PUT", &format!("/{BUCKET}"), &[], Vec::new()).await;
    assert_eq!(
        answer.status,
        200,
        "setup: the bucket must be created, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `PUT /<bucket>/<key>` — `PutObject`.
async fn given_object(router: &MultiServiceRouter, key: &str, body: &str) {
    let answer = send(
        router,
        "s3",
        "PUT",
        &format!("/{BUCKET}/{key}"),
        &[],
        body.as_bytes().to_vec(),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "setup: the object {key} must be stored, got {} {}",
        answer.status,
        answer.text()
    );
}

/// One IAM action (`awsQuery`).
async fn iam(router: &MultiServiceRouter, action: &str, parameters: &[(&str, &str)]) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let answer = send(
        router,
        "iam",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        body.into_bytes(),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "setup: IAM {action} must be served, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Hold the execution role in this account's IAM: it trusts the SageMaker service principal
/// and may read the training data and write the artifact.
async fn given_role(router: &MultiServiceRouter) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {
                "Service": service_com_amazonaws_sagemaker_sagemaker::state::SERVICE_PRINCIPAL,
            },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let permissions = json!({
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
    .to_string();
    iam(
        router,
        "CreateRole",
        &[("RoleName", ROLE), ("AssumeRolePolicyDocument", &trust)],
    )
    .await;
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE),
            ("PolicyName", "serving-data-access"),
            ("PolicyDocument", &permissions),
        ],
    )
    .await;
    format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}")
}

/// Comma-separated training records: `rows` records whose first column is the target and
/// whose remaining `features` columns are the features, the target a linear function of them.
fn training_data(rows: usize, features: usize) -> String {
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

/// Poll `reader` until the resource's `member` reaches `wanted`, answering the read that did.
async fn settled(
    router: &MultiServiceRouter,
    case: &str,
    reader: &str,
    body: &Value,
    member: &str,
    wanted: &str,
) -> Value {
    let deadline = Instant::now() + SETTLE_BUDGET;
    loop {
        let read = control_ok(router, reader, body).await;
        let status = read.get(member).and_then(Value::as_str).unwrap_or_default();
        if status == wanted {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {reader} never reported {member} {wanted:?}; last read {read}"
        );
        tokio::time::sleep(SETTLE_POLL).await;
    }
}

/// An ECR-shaped container image path in this account's registry. The emulator pulls no
/// image; the path is the reference the control plane records, as every other image reference
/// in these services is.
fn image() -> String {
    format!("{ACCOUNT}.dkr.ecr.{REGION}.amazonaws.com/serving:1")
}

/// Train a model on `features` feature columns and deploy it behind `endpoint` on the variant
/// `variant`, answering once the endpoint is `InService`.
async fn trained_and_deployed(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    endpoint: &str,
    variant: &str,
    features: usize,
) {
    let role_arn = given_role(router).await;
    let key = format!("{name}/train.csv");
    given_object(router, &key, &training_data(40, features)).await;
    control_ok(
        router,
        "CreateTrainingJob",
        &json!({
            "TrainingJobName": name,
            "AlgorithmSpecification": {
                "TrainingImage": image(),
                "TrainingInputMode": "File",
            },
            "RoleArn": role_arn,
            "InputDataConfig": [{
                "ChannelName": "train",
                "DataSource": {
                    "S3DataSource": {
                        "S3DataType": "S3Prefix",
                        "S3Uri": format!("s3://{BUCKET}/{key}"),
                    },
                },
                "ContentType": "text/csv",
            }],
            "OutputDataConfig": { "S3OutputPath": format!("s3://{BUCKET}/{name}/out/") },
            "ResourceConfig": {
                "InstanceType": "ml.m5.large",
                "InstanceCount": 1,
                "VolumeSizeInGB": 10,
            },
            "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
        }),
    )
    .await;
    let trained = settled(
        router,
        case,
        "DescribeTrainingJob",
        &json!({ "TrainingJobName": name }),
        "TrainingJobStatus",
        "Completed",
    )
    .await;
    let artifact = trained
        .pointer("/ModelArtifacts/S3ModelArtifacts")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the completed job names no artifact: {trained}"))
        .to_string();

    deploy(router, case, name, endpoint, variant, Some(&artifact)).await;
}

/// Deploy a model carrying `artifact` (when it names one) behind `endpoint` on `variant`, and
/// answer once the endpoint is `InService`.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    endpoint: &str,
    variant: &str,
    artifact: Option<&str>,
) {
    let role_arn = format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}");
    let mut container = json!({ "Image": image() });
    if let Some(artifact) = artifact {
        container["ModelDataUrl"] = json!(artifact);
    }
    control_ok(
        router,
        "CreateModel",
        &json!({
            "ModelName": name,
            "PrimaryContainer": container,
            "ExecutionRoleArn": role_arn,
        }),
    )
    .await;
    let config = format!("{name}-config");
    control_ok(
        router,
        "CreateEndpointConfig",
        &json!({
            "EndpointConfigName": config,
            "ProductionVariants": [{
                "VariantName": variant,
                "ModelName": name,
                "InstanceType": "ml.m5.large",
                "InitialInstanceCount": 1,
            }],
        }),
    )
    .await;
    control_ok(
        router,
        "CreateEndpoint",
        &json!({ "EndpointName": endpoint, "EndpointConfigName": config }),
    )
    .await;
    settled(
        router,
        case,
        "DescribeEndpoint",
        &json!({ "EndpointName": endpoint }),
        "EndpointStatus",
        "InService",
    )
    .await;
}

/// One row of the invocation table.
struct Row {
    case: &'static str,
    /// The request headers the invocation carries.
    headers: Vec<(&'static str, &'static str)>,
    /// The payload, carrying `records` records of [`TABLE_FEATURES`] feature values.
    payload: String,
    /// How many records the payload carries, which is how many predictions must come back.
    records: usize,
    /// The media type the answer must be rendered in.
    answer_media_type: &'static str,
}

/// One row's payload as comma-separated records: `records` rows of `features` values, each
/// row's values offset by its index so no two rows are the same record.
fn csv_payload(records: usize, features: usize) -> String {
    (0..records)
        .map(|record| {
            let values: Vec<String> = (0..features)
                .map(|column| format!("{}", (record * 10 + column + 1) as f64))
                .collect();
            format!("{}\n", values.join(","))
        })
        .collect()
}

/// The same records as a JSON list of lists.
fn json_rows(records: usize, features: usize) -> Vec<Vec<f64>> {
    (0..records)
        .map(|record| {
            (0..features)
                .map(|column| (record * 10 + column + 1) as f64)
                .collect()
        })
        .collect()
}

/// How many feature columns the model the table is driven against is fitted on. One model
/// serves every row: what the rows vary is the PAYLOAD and the media types, so training a
/// model per row would only repeat the same setup.
const TABLE_FEATURES: usize = 3;

/// The invocation table. Rows differ along the shape of the payload (comma-separated records,
/// a JSON list of records, a JSON `instances` list, a bare JSON row), how many records one
/// invocation carries, and the media type the answer is asked for (none, `text/csv`,
/// `application/json`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one comma-separated record, no Accept",
            headers: vec![("content-type", "text/csv")],
            payload: csv_payload(1, TABLE_FEATURES),
            records: 1,
            answer_media_type: "text/csv",
        },
        Row {
            case: "several comma-separated records, text/csv asked for",
            headers: vec![("content-type", "text/csv"), ("accept", "text/csv")],
            payload: csv_payload(4, TABLE_FEATURES),
            records: 4,
            answer_media_type: "text/csv",
        },
        Row {
            case: "several comma-separated records, application/json asked for",
            headers: vec![
                ("content-type", "text/csv"),
                ("accept", "application/json"),
            ],
            payload: csv_payload(2, TABLE_FEATURES),
            records: 2,
            answer_media_type: "application/json",
        },
        Row {
            case: "a JSON list of records, application/json asked for",
            headers: vec![
                ("content-type", "application/json"),
                ("accept", "application/json"),
            ],
            payload: json!(json_rows(3, TABLE_FEATURES)).to_string(),
            records: 3,
            answer_media_type: "application/json",
        },
        Row {
            case: "a JSON instances list, no Accept",
            headers: vec![("content-type", "application/json")],
            payload: json!({ "instances": json_rows(2, TABLE_FEATURES) }).to_string(),
            records: 2,
            answer_media_type: "text/csv",
        },
        Row {
            case: "one JSON record as a bare list of numbers",
            headers: vec![("content-type", "application/json")],
            payload: json!(json_rows(1, TABLE_FEATURES)[0]).to_string(),
            records: 1,
            answer_media_type: "text/csv",
        },
    ]
}

/// The predictions one answer carries, whichever media type it is rendered in.
fn predictions(answer: &Answer, case: &str) -> Vec<f64> {
    let media_type = answer.header("content-type");
    if media_type.contains("application/json") {
        return answer
            .json()
            .get("predictions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no predictions in {}", answer.text()))
            .iter()
            .map(|entry| {
                entry
                    .get("score")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| panic!("case {case:?}: no score in {entry}"))
            })
            .collect();
    }
    answer
        .text()
        .lines()
        .map(|line| {
            line.trim()
                .parse::<f64>()
                .unwrap_or_else(|error| panic!("case {case:?}: {line:?} is no number: {error}"))
        })
        .collect()
}

/// RULE ONE: an endpoint answers what the model behind it computes for the payload.
#[tokio::test]
async fn an_endpoint_serves_the_prediction_its_deployed_model_computes() {
    // One model, trained and deployed once, serves every row: the rows vary the payload and
    // the media types, not the deployment.
    let router = emulator();
    given_bucket(&router).await;
    let endpoint = "serving-endpoint";
    let variant = "AllTraffic";
    trained_and_deployed(
        &router,
        "the table's model",
        "serving-job",
        endpoint,
        variant,
        TABLE_FEATURES,
    )
    .await;

    for row in rows() {
        let case = row.case;
        let answer = invoke(&router, endpoint, &row.headers, row.payload.as_bytes()).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: the invocation must be served, got {} with {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.header("content-type"),
            row.answer_media_type,
            "case {case:?}: the answer's media type, got {}",
            answer.text()
        );
        assert_eq!(
            answer.header("x-amzn-invoked-production-variant"),
            variant,
            "case {case:?}: the answer must name the variant that served it"
        );
        let served = predictions(&answer, case);
        assert_eq!(
            served.len(),
            row.records,
            "case {case:?}: one prediction per record, got {served:?}"
        );
        for value in &served {
            assert!(
                value.is_finite(),
                "case {case:?}: every prediction is a computed number, got {served:?}"
            );
        }

        // The answer is COMPUTED: the same payload is answered the same way twice, so no
        // call changed what the next caller is served.
        let again = invoke(&router, endpoint, &row.headers, row.payload.as_bytes()).await;
        assert_eq!(
            predictions(&again, case),
            served,
            "case {case:?}: the same payload must be answered the same way"
        );
    }

    // And the answer DEPENDS on the features the payload carried: one record and the same
    // record with every feature shifted are answered differently.
    let case = "a record and the same record shifted";
    let record = csv_payload(1, TABLE_FEATURES);
    let shifted = record
        .trim_end()
        .split(',')
        .map(|value| format!("{}", value.parse::<f64>().expect("a number") + 100.0))
        .collect::<Vec<_>>()
        .join(",");
    let csv = [("content-type", "text/csv")];
    let served = predictions(&invoke(&router, endpoint, &csv, record.as_bytes()).await, case);
    let shifted_served =
        predictions(&invoke(&router, endpoint, &csv, shifted.as_bytes()).await, case);
    assert_ne!(
        shifted_served, served,
        "case {case:?}: records differing in their features must be answered differently"
    );
}

/// RULE TWO: where there is nothing to compute from, the modelled failure names the
/// condition — never a synthesised body.
#[tokio::test]
async fn an_invocation_nothing_can_compute_is_refused_with_model_error() {
    // A variant whose model names no model artifact: there are no parameters to serve.
    let router = emulator();
    given_bucket(&router).await;
    given_role(&router).await;
    deploy(
        &router,
        "no artifact",
        "artifactless-model",
        "artifactless-endpoint",
        "AllTraffic",
        None,
    )
    .await;
    let answer = invoke(
        &router,
        "artifactless-endpoint",
        &[("content-type", "text/csv")],
        b"1,2,3",
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code().as_str()),
        (424, "ModelError"),
        "case \"no artifact\": got {} with {}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.message(),
        "Model artifactless-model carries no ModelDataUrl, so the endpoint has no model \
         artifacts to serve an inference from.",
        "case \"no artifact\": the refusal must name the model that carries none"
    );

    // A model that WAS trained, invoked with payloads it cannot be applied to.
    let router = emulator();
    given_bucket(&router).await;
    let endpoint = "trained-endpoint";
    trained_and_deployed(
        &router,
        "trained",
        "trained-job",
        endpoint,
        "AllTraffic",
        TABLE_FEATURES,
    )
    .await;

    // Rows: a record of the wrong width, and a payload that holds no record at all.
    let narrow = "1,2".to_string();
    let mismatch = format!(
        "The request payload's records carry 2 feature values, and the model behind this \
         endpoint was fitted on {TABLE_FEATURES}."
    );
    let refusals: Vec<(&str, &[u8], String)> = vec![
        ("a record of the wrong width", narrow.as_bytes(), mismatch),
        (
            "a payload holding no record",
            b"not,a,record",
            "The request payload holds no record to predict from: a record is a row of \
             numeric feature values, as text/csv or as a JSON array of them."
                .to_string(),
        ),
        (
            "an empty payload",
            b"",
            "The request payload holds no record to predict from: a record is a row of \
             numeric feature values, as text/csv or as a JSON array of them."
                .to_string(),
        ),
    ];
    for (case, payload, expected) in refusals {
        let answer = invoke(&router, endpoint, &[("content-type", "text/csv")], payload).await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (424, "ModelError"),
            "case {case:?}: got {} with {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: the refusal must name the condition that stopped the inference"
        );
    }
}
