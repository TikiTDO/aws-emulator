// SPDX-License-Identifier: CC0-1.0

//! That a WELL-FORMED batch inference submission is accepted — asserted end to end with this
//! emulator's own Amazon S3 mounted beside the control plane on one engine, the way the
//! assembled emulator runs them, so the input the submission names is really staged and the
//! job it mints is really read back.
//!
//! THE RULE UNDER TEST: a `CreateModelInvocationJob` request whose members all satisfy their
//! modelled constraints and whose `modelId` names a model this service knows is ACCEPTED on
//! the FIRST attempt — HTTP 200 carrying a `jobArn` of the modelled form
//! (`ModelInvocationJobArn`:
//! `^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:model-invocation-job/[a-z0-9]{12})$`,
//! `smithy_query(query="operation",operation="CreateModelInvocationJob")`) — and the job it
//! mints reads back through `GetModelInvocationJob` in a status
//! `ModelInvocationJobStatus` declares, no failure among them, and appears in
//! `ListModelInvocationJobs`. None of the modelled 4xx answers is a correct answer to such a
//! request: a caller that submits one and then polls must never be left retrying a refusal.
//! A submission that DOES break a modelled constraint is refused 400 `ValidationException`
//! and mints nothing, and the identically shaped well-formed submission that follows it is
//! accepted all the same — a refusal leaves no state behind that could make the next
//! submission fail.
//!
//! The rows span what the rule generalises over: the SPELLING of `modelId` (a bare published
//! id, the `foundation-model` ARN of the same model, a version-qualified id, a model of
//! another provider), the optional members the request carries (none, an idempotency token, a
//! timeout, tags, an invocation endpoint), the shape of the input key and of the output
//! prefix (a prefix, a nested prefix, the bucket itself), and — for the refused first attempt
//! — which way its `s3Uri` breaks `S3Uri`'s declared pattern (a different scheme, no scheme,
//! an upper-case bucket, a bucket shorter than the pattern admits).
//!
//! Beside the table, the whole published catalogue: every base model the catalogue publishes
//! batch inference for is submitted by its `foundation-model` ARN, because a caller reads a
//! model id out of `ListFoundationModels` and submits it — so acceptance must hold for every
//! row the catalogue carries rather than for a chosen one.
//!
//! The emulator, the staged bucket, the input objects and the job read are the harness
//! `crate::model_invocation_job_execution` owns, reused rather than copied: one dispatch
//! path for one rule.

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::model_invocation_job_execution::{
    emulator, given_bucket, given_input_object, read_job, record, Emulator, BUCKET, REGION,
};
use crate::support::{Reply, ACCOUNT};

/// The `RoleArn` every submission hands Amazon Bedrock: a same-account service role, the
/// shape `RoleArn` declares (`^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
fn batch_role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/BatchInferenceServiceRole")
}

/// `S3Uri`'s declared `@pattern` (`smithy_query(query="shape",shape="S3Uri")`), anchors
/// included as the model declares them.
const S3_URI_PATTERN: &str =
    r"^s3://[a-z0-9][-.a-z0-9]{1,61}[a-z0-9](?:/[-!_*'().a-z0-9A-Z]+(?:/[-!_*'().a-z0-9A-Z]+)*)?/?$";

/// The member path the refused first attempt breaks its constraint at: the `s3Uri` inside the
/// input configuration's S3 variant.
const INPUT_URI_MEMBER: &str = "inputDataConfig.s3InputDataConfig.s3Uri";

/// The statuses `ModelInvocationJobStatus` declares that are not a failure of the job
/// (`smithy_query(query="shape",shape="ModelInvocationJobStatus")` declares `Submitted`,
/// `InProgress`, `Completed`, `Failed`, `Stopping`, `Stopped`, `PartiallyCompleted`,
/// `Expired`, `Validating`, `Scheduled`) — the ones an accepted submission's job may report
/// while nobody has stopped it and its timeout stands a day away.
const STATUSES_OF_AN_ACCEPTED_JOB: &[&str] = &[
    "Submitted",
    "Validating",
    "Scheduled",
    "InProgress",
    "Completed",
    "PartiallyCompleted",
];

/// What a constraint refusal of the input URI states: the framework's envelope over the
/// expression the WHOLE value must match, which is the declared pattern without the anchors
/// the model makes the match total with.
fn input_uri_refusal() -> String {
    format!(
        "1 validation error detected: Value at '{INPUT_URI_MEMBER}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {}",
        S3_URI_PATTERN.trim_start_matches('^').trim_end_matches('$')
    )
}

/// The `message` a reply carries, or the empty string when it carries none.
fn message(reply: &Reply) -> String {
    reply
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One submission: how the row names its model, what it stages, what it asks for, and the
/// malformed `s3Uri` its first attempt carries.
struct Row {
    case: String,
    /// `jobName`, which is also what the listing is searched by.
    name: String,
    /// `modelId`, in the spelling this row uses.
    model_id: String,
    /// The key the input object is staged under.
    input_key: &'static str,
    /// The `outputDataConfig.s3OutputDataConfig.s3Uri` suffix after the bucket: a prefix, a
    /// nested prefix, or nothing at all (the bucket itself).
    output_suffix: &'static str,
    /// The optional members this row carries beside the required ones.
    extras: Value,
    /// The `s3Uri` the row's REFUSED first attempt carries, and how it breaks the pattern.
    malformed_uri: &'static str,
}

impl Row {
    fn input_uri(&self) -> String {
        format!("s3://{BUCKET}/{}", self.input_key)
    }

    fn output_uri(&self) -> String {
        format!("s3://{BUCKET}{}", self.output_suffix)
    }

    /// The request body, with `input_uri` standing where the row's input URI goes — the one
    /// place the refused attempt and the well-formed one differ.
    fn body(&self, input_uri: &str) -> Value {
        let mut body = json!({
            "jobName": self.name,
            "roleArn": batch_role(),
            "modelId": self.model_id,
            "inputDataConfig": {
                "s3InputDataConfig": { "s3InputFormat": "JSONL", "s3Uri": input_uri }
            },
            "outputDataConfig": { "s3OutputDataConfig": { "s3Uri": self.output_uri() } },
        });
        for (member, value) in self.extras.as_object().into_iter().flatten() {
            body[member] = value.clone();
        }
        body
    }
}

/// A base model the published catalogue offers batch inference for: `features_supported`
/// carries `BATCH_INFERENCE`, which is the per-model fact the catalogue reports as
/// `batchSupported.baseModelSupported`.
fn batch_models() -> Vec<&'static bedrock::state::FoundationModel> {
    bedrock::state::FOUNDATION_MODELS
        .iter()
        .filter(|model| model.features_supported.contains(&"BATCH_INFERENCE"))
        .collect()
}

/// The `FoundationModelArn` of `model_id` in [`REGION`]: AWS publishes a foundation model, so
/// its ARN carries an EMPTY account field.
fn foundation_model_arn(model_id: &str) -> String {
    format!("arn:aws:bedrock:{REGION}::foundation-model/{model_id}")
}

/// The rows: each a well-formed submission, picked off the published catalogue by a PROPERTY
/// rather than by a recorded model id, and each carrying its own way of breaking the input
/// URI's pattern for the attempt that precedes it.
fn rows() -> Vec<Row> {
    let models = batch_models();
    assert!(
        models.len() >= 2,
        "the catalogue must publish batch inference for at least two models, got {}",
        models.len()
    );
    let first = models[0];
    // A model of another provider, and one whose published id carries a version qualifier —
    // both spellings `ModelId` admits for a base model.
    let other_provider = models
        .iter()
        .find(|model| model.provider_name != first.provider_name)
        .copied()
        .unwrap_or(models[1]);
    let qualified = models
        .iter()
        .find(|model| model.model_id.contains(':'))
        .copied()
        .unwrap_or(first);

    vec![
        Row {
            case: format!("{} by its foundation-model ARN", first.model_id),
            name: "batchacceptedarn".to_string(),
            model_id: foundation_model_arn(first.model_id),
            input_key: "input/records.jsonl",
            output_suffix: "/output/",
            extras: json!({}),
            malformed_uri: "ftp://not-a-valid-s3-uri/records.jsonl",
        },
        Row {
            case: format!("{} by its bare published id", first.model_id),
            name: "batchacceptedbareid".to_string(),
            model_id: first.model_id.to_string(),
            input_key: "nested/input/records.jsonl",
            output_suffix: "/nested/output/",
            extras: json!({ "modelInvocationType": "InvokeModel" }),
            malformed_uri: "/input/records.jsonl",
        },
        Row {
            case: format!("{} with a version qualifier, by its ARN", qualified.model_id),
            name: "batchacceptedqualified".to_string(),
            model_id: foundation_model_arn(qualified.model_id),
            input_key: "input/qualified.jsonl",
            output_suffix: "",
            extras: json!({
                "clientRequestToken": "batchacceptedqualifiedtoken",
                "timeoutDurationInHours": 24,
            }),
            malformed_uri: "s3://INPUT-BUCKET/input/records.jsonl",
        },
        Row {
            case: format!("{} of another provider, by its ARN", other_provider.model_id),
            name: "batchacceptedotherprovider".to_string(),
            model_id: foundation_model_arn(other_provider.model_id),
            input_key: "input/other-provider.jsonl",
            output_suffix: "/output/other-provider/",
            extras: json!({
                "tags": [{ "key": "suite", "value": "batch-submission-acceptance" }],
                "modelInvocationType": "Converse",
            }),
            malformed_uri: "s3://a/input/records.jsonl",
        },
    ]
}

/// The `jobArn` an accepted submission answers with is of the modelled form: this account and
/// Region, the `model-invocation-job` resource type, and a minted id of twelve lower-case
/// base-36 digits.
fn assert_job_arn(case: &str, arn: &str) {
    let prefix = format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:model-invocation-job/");
    assert!(
        arn.starts_with(&prefix),
        "case {case:?}: expected a jobArn under {prefix}, got {arn:?}"
    );
    let id = &arn[prefix.len()..];
    assert!(
        id.chars().count() == 12
            && id
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
        "case {case:?}: expected a minted id of twelve lower-case base-36 digits, got {id:?}"
    );
}

/// How many jobs the listing reports under `nameContains`.
async fn jobs_named(emu: &Emulator, name: &str) -> Vec<Value> {
    let listed = emu
        .bedrock(
            "GET",
            &format!("/model-invocation-jobs?nameContains={name}"),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {name:?}: the listing must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    listed
        .get("invocationJobSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// RULE: a refused submission mints nothing, and the identically shaped well-formed
/// submission is then accepted on the FIRST attempt with a `jobArn` whose job reads back in a
/// modelled, non-failed status and appears in the listing.
#[tokio::test]
async fn a_well_formed_submission_is_accepted_on_the_first_attempt() {
    let emu = emulator();
    given_bucket(&emu).await;

    for row in rows() {
        let case = &row.case;
        given_input_object(
            &emu,
            row.input_key,
            &[record(
                Some("RECORD00001"),
                json!({ "inputText": "a prompt staged by the acceptance suite" }),
            )],
        )
        .await;

        // The attempt that breaks `S3Uri`'s pattern: refused for the input it names, and it
        // mints nothing.
        let refused = emu
            .bedrock(
                "POST",
                "/model-invocation-job",
                Some(row.body(row.malformed_uri)),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?} ({:?}): a malformed s3Uri is an input fault, got {} {:?}",
            row.malformed_uri,
            refused.status,
            refused.body
        );
        assert_eq!(
            message(&refused),
            input_uri_refusal(),
            "case {case:?} ({:?}): expected the constraint envelope over the member it \
             broke, got {:?}",
            row.malformed_uri,
            refused.body
        );
        assert_eq!(
            refused.get("jobArn"),
            Value::Null,
            "case {case:?}: a refused submission answers with no jobArn, got {:?}",
            refused.body
        );
        assert!(
            jobs_named(&emu, &row.name).await.is_empty(),
            "case {case:?}: a refused submission mints no job, got {:?}",
            jobs_named(&emu, &row.name).await
        );

        // The same request, well formed: accepted on the FIRST attempt — this is one call,
        // not a retry loop.
        let accepted = emu
            .bedrock(
                "POST",
                "/model-invocation-job",
                Some(row.body(&row.input_uri())),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: a well-formed submission must be accepted on the first attempt, \
             got {} {:?}",
            accepted.status, accepted.body
        );
        let job_arn = accepted.text("jobArn");
        assert_job_arn(case, &job_arn);

        // Round trip: the job the submission minted reads back under that ARN, reporting what
        // the request asked for and a status that is no failure.
        let read = read_job(&emu, &job_arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the submitted job must read back, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("jobArn"),
            job_arn,
            "case {case:?}: the read must report the ARN the submission answered with, got \
             {:?}",
            read.body
        );
        assert_eq!(
            read.text("modelId"),
            row.model_id,
            "case {case:?}: the read must report the modelId the submission named, got {:?}",
            read.body
        );
        assert_eq!(
            read.text("roleArn"),
            batch_role(),
            "case {case:?}: the read must report the role the submission passed, got {:?}",
            read.body
        );
        let status = read.text("status");
        assert!(
            STATUSES_OF_AN_ACCEPTED_JOB.contains(&status.as_str()),
            "case {case:?}: expected one of {STATUSES_OF_AN_ACCEPTED_JOB:?}, got {status:?} \
             with {:?}",
            read.body
        );
        assert_eq!(
            read.get("message"),
            Value::Null,
            "case {case:?}: an accepted submission's job carries no failure message, got {:?}",
            read.body
        );

        // And the listing a caller finds its job through reports exactly that one job.
        let listed = jobs_named(&emu, &row.name).await;
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: the listing must report the one job the submission minted, got \
             {listed:?}"
        );
        assert_eq!(
            listed[0].get("jobArn").and_then(Value::as_str),
            Some(job_arn.as_str()),
            "case {case:?}: the listed job must be the one that was minted, got {listed:?}"
        );
    }
}

/// RULE, over the whole published catalogue: a caller reads a model id out of
/// `ListFoundationModels` and submits a batch job for it, so EVERY base model the catalogue
/// publishes batch inference for must resolve when named by its `foundation-model` ARN and
/// the submission must be accepted — a catalogue row a submission cannot name is a model a
/// client cannot run a batch job on.
///
/// The submissions are made in chunks, each against its own emulator, because an account
/// holds at most [`bedrock::state::MAX_MODEL_INVOCATION_JOBS_PER_ACCOUNT`] batch inference
/// jobs and the rule under test is acceptance rather than that quota.
#[tokio::test]
async fn every_model_the_catalogue_offers_for_batch_inference_is_accepted_by_its_arn() {
    const INPUT_KEY: &str = "input/catalogue.jsonl";
    let chunk_size = bedrock::state::MAX_MODEL_INVOCATION_JOBS_PER_ACCOUNT / 2;
    let models = batch_models();
    assert!(
        !models.is_empty(),
        "the catalogue must publish batch inference for at least one model"
    );

    let mut submitted = 0usize;
    for chunk in models.chunks(chunk_size) {
        let emu = emulator();
        given_bucket(&emu).await;
        given_input_object(
            &emu,
            INPUT_KEY,
            &[record(
                Some("RECORD00001"),
                json!({ "inputText": "a prompt staged by the acceptance suite" }),
            )],
        )
        .await;

        for model in chunk {
            let case = format!("the catalogued model {}", model.model_id);
            let name = format!("batchcatalogue{submitted}");
            submitted += 1;
            let reply = emu
                .bedrock(
                    "POST",
                    "/model-invocation-job",
                    Some(json!({
                        "jobName": name,
                        "roleArn": batch_role(),
                        "modelId": foundation_model_arn(model.model_id),
                        "inputDataConfig": {
                            "s3InputDataConfig": {
                                "s3InputFormat": "JSONL",
                                "s3Uri": format!("s3://{BUCKET}/{INPUT_KEY}"),
                            }
                        },
                        "outputDataConfig": {
                            "s3OutputDataConfig": { "s3Uri": format!("s3://{BUCKET}/output/") }
                        },
                    })),
                )
                .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: the catalogue publishes batch inference for this model, so \
                 the submission naming its ARN must be accepted, got {} {:?}",
                reply.status, reply.body
            );
            let job_arn = reply.text("jobArn");
            assert_job_arn(&case, &job_arn);

            let read = read_job(&emu, &job_arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: the submitted job must read back, got {} {:?}",
                read.status, read.body
            );
            let status = read.text("status");
            assert!(
                STATUSES_OF_AN_ACCEPTED_JOB.contains(&status.as_str()),
                "case {case:?}: expected one of {STATUSES_OF_AN_ACCEPTED_JOB:?}, got \
                 {status:?} with {:?}",
                read.body
            );
        }
    }
}
