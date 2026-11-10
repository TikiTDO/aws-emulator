// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` — removing tags from a resource this service holds — and the wording of
//! two refusals whose text is the service's own, end to end over the wire the service
//! speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rules under test:
//!
//! * `UntagResource` "[r]emoves tags from an Amazon Web Services resource": the keys the
//!   request names are dropped from the resource's tag set and every other key it holds is
//!   left alone, so the operation is idempotent in the key it names — removing a key the
//!   resource does not carry changes nothing and is not refused — and the result is read
//!   back through `ListTagsForResource`;
//! * it resolves the resource FIRST, so a string that is not a `codepipeline` resource ARN
//!   is `InvalidArnException`, a well-formed ARN naming no resource this service holds is
//!   `ResourceNotFoundException`, and a key list no resource can be asked about is
//!   `InvalidTagsException` — each a 400, and each leaving the stored tags untouched;
//! * `PutWebhook` refuses a `jsonPath` that is not a JsonPath expression with the sentence
//!   that refusal states, over the expression the request sent;
//! * `RetryStageExecution` refuses a stage that cannot be retried with the one sentence
//!   that states every reason it cannot, over the stage, the pipeline and the execution the
//!   request named.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    invalid_filter_pattern_message, stage_not_retryable_message,
};

use crate::pipeline_lifecycle::{creation_run, declaration, Emulator, ACCOUNT_ID};

/// The status every error these operations model defaults to.
const REFUSAL_STATUS: u16 = 400;

/// The ARN of the pipeline `name` in `region`, as this service mints it.
fn pipeline_arn(region: &str, name: &str) -> String {
    format!("arn:aws:codepipeline:{region}:{ACCOUNT_ID}:{name}")
}

/// The tags a resource holds, read back through the listing.
async fn listed_tags(emulator: &Emulator, arn: &str) -> BTreeMap<String, String> {
    let listed = emulator
        .ok("ListTagsForResource", &json!({"resourceArn": arn}))
        .await;
    listed["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `(status, code, message)` of a refusal, through the one reader the shared harness
/// owns (`Emulator::refusal`).
async fn refusal(emulator: &Emulator, operation: &str, body: &Value) -> (u16, String, String) {
    emulator.refusal(operation, body).await
}

/// The tag set every row of the removal table starts from.
fn initial_tags() -> Vec<(&'static str, &'static str)> {
    vec![
        ("env", "prod"),
        ("owner", "platform"),
        ("team", "build"),
        ("cost:center", "4711"),
    ]
}

/// An emulator holding the pipeline `name` tagged with [`initial_tags`], and that
/// pipeline's ARN.
async fn emulator_with_tagged_pipeline(name: &str) -> (Emulator, String) {
    let emulator = Emulator::new();
    let created = emulator
        .ok("CreatePipeline", &json!({"pipeline": declaration(name)}))
        .await;
    let arn = pipeline_arn("us-east-1", name);
    let _ = created;
    emulator
        .ok(
            "TagResource",
            &json!({
                "resourceArn": arn,
                "tags": initial_tags()
                    .into_iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
    (emulator, arn)
}

/// RULE: `UntagResource` drops the keys the request names from the resource's tag set and
/// leaves every other key it holds alone, whether or not the resource carried them.
#[tokio::test]
async fn untagging_removes_the_named_keys_and_leaves_the_rest() {
    // The rows differ along what the rule generalises over: how many keys are named, where
    // in the set they sit, whether the resource carries them at all, and the whole set.
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("no key at all", vec![]),
        ("one key of several", vec!["env"]),
        ("another single key", vec!["team"]),
        ("two keys at once", vec!["env", "owner"]),
        ("a key the resource does not carry", vec!["absent"]),
        ("one held key and one absent one", vec!["team", "absent"]),
        (
            "a key whose name carries a colon",
            vec!["cost:center"],
        ),
        (
            "every key the resource holds",
            vec!["env", "owner", "team", "cost:center"],
        ),
    ];

    for (case, keys) in rows {
        let (emulator, arn) = emulator_with_tagged_pipeline("demo").await;
        emulator
            .ok(
                "UntagResource",
                &json!({"resourceArn": arn, "tagKeys": keys}),
            )
            .await;
        let expected: BTreeMap<String, String> = initial_tags()
            .into_iter()
            .filter(|(key, _)| !keys.contains(key))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let actual = listed_tags(&emulator, &arn).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the tags {expected:?} to remain, got {actual:?}"
        );

        // Removal is idempotent: the same call again answers the same way and the set is
        // the same afterwards.
        emulator
            .ok(
                "UntagResource",
                &json!({"resourceArn": arn, "tagKeys": keys}),
            )
            .await;
        let again = listed_tags(&emulator, &arn).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second removal of the same keys changes nothing, got \
             {again:?}"
        );
    }
}

/// RULE: `UntagResource` resolves what the request names before it removes anything, and
/// each class of defect is refused with the error the model declares for it, changing
/// nothing.
#[tokio::test]
async fn an_untag_request_naming_nothing_removable_is_refused() {
    // (case, resourceArn, tagKeys, expected error code)
    let rows: Vec<(&str, String, Vec<&str>, &str)> = vec![
        (
            "a pipeline name no pipeline uses",
            pipeline_arn("us-east-1", "absent"),
            vec!["env"],
            "ResourceNotFoundException",
        ),
        (
            "the held pipeline's name in another Region",
            pipeline_arn("eu-west-1", "demo"),
            vec!["env"],
            "ResourceNotFoundException",
        ),
        (
            "a webhook no webhook definition holds",
            format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:webhook:absent"),
            vec!["env"],
            "ResourceNotFoundException",
        ),
        (
            "an action type this account has not registered",
            format!(
                "arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:actiontype:Custom/Build/My-Tool/1"
            ),
            vec!["env"],
            "ResourceNotFoundException",
        ),
        (
            "a region segment that is not a Region",
            pipeline_arn("US-EAST-1", "demo"),
            vec!["env"],
            "InvalidArnException",
        ),
        (
            "an action type identifier missing its version",
            format!(
                "arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:actiontype:Custom/Build/My-Tool"
            ),
            vec!["env"],
            "InvalidArnException",
        ),
        (
            "a key in the namespace AWS reserves",
            pipeline_arn("us-east-1", "demo"),
            vec!["aws:cloudformation:stack-name"],
            "InvalidTagsException",
        ),
        (
            "the same key named twice",
            pipeline_arn("us-east-1", "demo"),
            vec!["env", "env"],
            "InvalidTagsException",
        ),
    ];

    for (case, arn, keys, expected) in rows {
        let (emulator, held) = emulator_with_tagged_pipeline("demo").await;
        let before = listed_tags(&emulator, &held).await;
        let (status, code, message) = refusal(
            &emulator,
            "UntagResource",
            &json!({"resourceArn": arn, "tagKeys": keys}),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, expected),
            "case {case:?}: expected a {REFUSAL_STATUS} {expected}, got {status} {code} \
             ({message:?})"
        );
        let after = listed_tags(&emulator, &held).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused removal changes no tag, got {after:?}"
        );
    }
}

/// RULE: `PutWebhook` refuses a filter pattern that is not a JsonPath expression with the
/// sentence that refusal states, carrying the expression the request sent.
#[tokio::test]
async fn an_unparseable_filter_pattern_is_refused_with_the_sentence_that_states_it() {
    // The rows differ along the form of the expression that cannot be parsed.
    let patterns = [
        "$.[[not-a-json-path",
        "not-a-json-path",
        "ref",
        "$.",
        "$.commits[0",
        "$..",
    ];
    for pattern in patterns {
        let emulator = Emulator::new();
        let body = json!({"webhook": {
            "name": "hook",
            "targetPipeline": "demo",
            "targetAction": "Src",
            "filters": [{"jsonPath": pattern, "matchEquals": "x"}],
            "authentication": "UNAUTHENTICATED",
            "authenticationConfiguration": {},
        }});
        let (status, code, message) = refusal(&emulator, "PutWebhook", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "InvalidWebhookFilterPatternException"),
            "case {pattern:?}: unexpected refusal ({message:?})"
        );
        assert_eq!(
            message,
            invalid_filter_pattern_message(pattern),
            "case {pattern:?}: expected the filter-pattern sentence, got {message:?}"
        );
    }

    // The sentence itself, spelled out once: a drift in the builder's own wording is a
    // defect, so one row states the whole text rather than only that the builder was used.
    let emulator = Emulator::new();
    let (_, _, message) = refusal(
        &emulator,
        "PutWebhook",
        &json!({"webhook": {
            "name": "hook",
            "targetPipeline": "demo",
            "targetAction": "Src",
            "filters": [{"jsonPath": "$.[[not-a-json-path", "matchEquals": "x"}],
            "authentication": "UNAUTHENTICATED",
            "authenticationConfiguration": {},
        }}),
    )
    .await;
    assert_eq!(
        message, "Request contains invalid filter pattern: $.[[not-a-json-path",
        "expected the sentence this refusal states, got {message:?}"
    );
}

/// RULE: `RetryStageExecution` refuses a stage that cannot be retried now with the one
/// sentence that states every reason it cannot, over the stage, the pipeline and the
/// execution the request named.
#[tokio::test]
async fn a_stage_that_cannot_be_retried_is_refused_with_the_sentence_that_states_it() {
    // The rows differ along the pipeline, the stage of it addressed and the retry mode:
    // none of its stages has a failed action, so every row is refused by the same rule.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("demo", "Source", "FAILED_ACTIONS"),
        ("demo", "Build", "FAILED_ACTIONS"),
        ("demo", "Build", "ALL_ACTIONS"),
        ("other-pipeline", "Source", "ALL_ACTIONS"),
    ];
    for (pipeline, stage, retry_mode) in rows {
        let emulator = Emulator::new();
        emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration(pipeline)}))
            .await;
        let execution = creation_run(&emulator, pipeline).await;
        let (status, code, message) = refusal(
            &emulator,
            "RetryStageExecution",
            &json!({
                "pipelineName": pipeline,
                "stageName": stage,
                "pipelineExecutionId": execution,
                "retryMode": retry_mode,
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "StageNotRetryableException"),
            "case {pipeline}/{stage}/{retry_mode}: unexpected refusal ({message:?})"
        );
        assert_eq!(
            message,
            stage_not_retryable_message(pipeline, stage, &execution),
            "case {pipeline}/{stage}/{retry_mode}: expected the not-retryable sentence, \
             got {message:?}"
        );
        // The sentence itself, spelled out once per rule, so a drift in the builder's own
        // wording is caught rather than re-asserted: its fixed text exactly, with the
        // stage, the pipeline and the execution the request named filled in.
        if (pipeline, stage, retry_mode) == ("demo", "Build", "FAILED_ACTIONS") {
            assert_eq!(
                message,
                format!(
                    "The stage named Build in the demo pipeline can't be retried now. \
                     Either it contains no failed actions, the execution '{execution}' is \
                     already superseded, or the pipeline definition has changed since the \
                     action(s) failed."
                ),
                "expected the sentence this refusal states, got {message:?}"
            );
        }
    }
}

/// RULE: the same refusal states a SUPERSEDED execution — one a later run of the pipeline
/// replaced without failing at the stage named — because that is one of the conditions the
/// sentence covers, over the execution the request named.
#[tokio::test]
async fn a_superseded_execution_is_refused_by_the_same_not_retryable_rule() {
    // The rows differ along how many later runs superseded the one the request names, and
    // which stage of it the retry addresses.
    let rows: Vec<(&str, &str, usize)> = vec![
        ("one later run", "Source", 1),
        ("one later run, another stage", "Build", 1),
        ("two later runs", "Build", 2),
    ];
    for (case, stage, later_runs) in rows {
        let emulator = Emulator::new();
        let pipeline = "superseded";
        emulator
            .ok("CreatePipeline", &json!({"pipeline": declaration(pipeline)}))
            .await;
        // The execution the pipeline's own creation ran, which the later starts supersede.
        let superseded = creation_run(&emulator, pipeline).await;
        for _ in 0..later_runs {
            emulator
                .ok("StartPipelineExecution", &json!({"name": pipeline}))
                .await;
        }
        let (status, code, message) = refusal(
            &emulator,
            "RetryStageExecution",
            &json!({
                "pipelineName": pipeline,
                "stageName": stage,
                "pipelineExecutionId": superseded,
                "retryMode": "FAILED_ACTIONS",
            }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, "StageNotRetryableException"),
            "case {case:?}: unexpected refusal ({message:?})"
        );
        assert_eq!(
            message,
            stage_not_retryable_message(pipeline, stage, &superseded),
            "case {case:?}: expected the not-retryable sentence, got {message:?}"
        );
    }
}
