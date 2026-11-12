// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for what this service's READ paths report about a resource's own
//! collections and identities.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! is asserted is what a real SDK client gets.
//!
//! The rule under test: a read reports every member the resource ITSELF holds a value for
//! — a collection it owns, as the EMPTY map or list when it holds nothing, and a
//! `UserContext` the service records for it, as the EMPTY structure until a call has put an
//! identity in it — rather than omitting the member because the creating request did not
//! mention it. A member the resource holds no value for AT ALL (the source job of an entity
//! created through the API, the instant a run that never started began) stays absent, which
//! is a different thing, so the tables below assert both.
//!
//! The service-resolved scalars and structures of the same class — a configuration's
//! isolation flag, its utilization metrics — are pinned by `service_resolved_members`, and
//! the identities an UPDATE records by `experiment_tracking_updates` and
//! `model_card_updates`.

use std::time::Duration;

use serde_json::{json, Value};

// The crate's wire-test harness, exposed `pub(crate)` by the suite that introduced it so
// every sibling module in this one test binary shares one emulator and one request builder.
use crate::inference_deployment_lifecycle::{one_variant_config, primary_container, str_field, Emulator};

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`
/// (smithy_query operation=CreateAIRecommendationJob).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerReadRole";

/// How SageMaker words the `FailureReason` of a recommendation job a caller stopped: a
/// fixed sentence carrying nothing of the request.
const STOPPED_BY_USER: &str = "Job stopped by user request";

/// Long enough to pass `state::AI_RECOMMENDATION_JOB_STOPS_AFTER` (300 ms).
const PAST_STOP_DUE: Duration = Duration::from_millis(550);

/// Long enough to pass `state::AI_RECOMMENDATION_JOB_ENDS_AFTER` (400 ms), after which a
/// job left to run has reached its terminal status.
const PAST_JOB_END_DUE: Duration = Duration::from_millis(700);

/// What a member must look like in a read response.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// A map or structure present with exactly this many entries — `0` is the empty `{}`
    /// a resource holding nothing in that collection reports.
    Map(usize),
    /// A list present with exactly this many elements — `0` is the empty `[]`.
    List(usize),
    /// The member is ABSENT: the resource holds no value for it at all.
    Absent,
}

/// The member at `path` (dotted, e.g. `Source.SourceTypes`) of a read response.
///
/// Resolved through `serde_json`'s own JSON-pointer walk rather than a hand-rolled one,
/// so no second path walker lives here (`service_resolved_members` reads its members the
/// same way).
fn member<'a>(body: &'a Value, path: &str) -> Option<&'a Value> {
    body.pointer(&format!("/{}", path.replace('.', "/")))
}

/// Assert one member of a read response against the rule, naming the case and the value
/// actually served.
fn check(case: &str, body: &Value, path: &str, expect: Expect) {
    let found = member(body, path);
    match expect {
        Expect::Map(entries) => assert!(
            found
                .and_then(Value::as_object)
                .is_some_and(|map| map.len() == entries),
            "case {case:?}: expected {path} to be a map of {entries} entries, got {found:?}"
        ),
        Expect::List(elements) => assert!(
            found
                .and_then(Value::as_array)
                .is_some_and(|list| list.len() == elements),
            "case {case:?}: expected {path} to be a list of {elements} elements, got {found:?}"
        ),
        Expect::Absent => assert!(
            found.is_none(),
            "case {case:?}: expected {path} to be absent, got {found:?}"
        ),
    }
}

/// One resource, created as the row describes it and read back through its own reader.
struct Row {
    case: &'static str,
    /// The calls that bring the resource into being, in order; each must answer 200.
    setup: Vec<(&'static str, Value)>,
    /// The reader under test.
    read: &'static str,
    /// Its request, built from the LAST setup response, so a lineage entity is addressed
    /// by the ARN its create minted.
    request: fn(&Value) -> Value,
    /// What the read must report, member by member.
    expect: Vec<(&'static str, Expect)>,
}

/// Run every row on its own emulator, asserting each row's expectations.
async fn run(rows: Vec<Row>) {
    for row in rows {
        let emulator = Emulator::new();
        let mut created = Value::Null;
        for (operation, request) in &row.setup {
            created = emulator.ok(operation, request).await;
        }
        let described = emulator.ok(row.read, &(row.request)(&created)).await;
        for (path, expect) in &row.expect {
            check(row.case, &described, path, *expect);
        }
    }
}

/// A tag list of `count` distinct key/value pairs, as a create carries one.
fn tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("key-{index}"), "Value": format!("v{index}") }))
            .collect(),
    )
}

/// A `CreateModelCard` `Content` document: the content must be "in model card JSON schema
/// and provided as a string", so the member is a JSON object rendered as a string.
fn card_content(name: &str) -> String {
    json!({ "model_overview": { "model_name": name } }).to_string()
}

/// A `CreateAIRecommendationJob` body carrying the operation's required members and
/// whatever `extra` adds on top of them.
fn recommendation_job(name: &str, workload: &str, extra: Value) -> Value {
    let mut body = json!({
        "AIRecommendationJobName": name,
        "AIWorkloadConfigIdentifier": workload,
        "RoleArn": ROLE_ARN,
        "ModelSource": { "S3": { "S3Uri": "s3://bucket/models/llama/" } },
        "OutputConfig": { "S3OutputLocation": "s3://bucket/recommendations/" },
        "PerformanceTarget": { "Constraints": [{ "Metric": "ttft-ms" }] },
    });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (key, value) in extra.as_object().expect("the extra members are an object") {
        object.insert(key.clone(), value.clone());
    }
    body
}

/// RULE: a collection a resource owns is reported by its reader whatever is in it — the
/// EMPTY map or list for a resource created with no entries — because the member is how
/// the resource reports what it holds rather than what the request mentioned.
///
/// The rows span the collection kinds the rule covers (a map of strings, a map of
/// structures, a list, a list nested inside a structure, a tag list) and the sizes it
/// generalises over: none, one and several entries.
#[tokio::test]
async fn an_entitys_own_collection_is_reported_as_the_empty_one_rather_than_omitted() {
    let rows = vec![
        Row {
            case: "an artifact created with no properties and no source types",
            setup: vec![(
                "CreateArtifact",
                json!({
                    "ArtifactName": "artifact-bare",
                    "ArtifactType": "DataSet",
                    "Source": { "SourceUri": "s3://bucket/bare" },
                }),
            )],
            read: "DescribeArtifact",
            request: |created| json!({ "ArtifactArn": created["ArtifactArn"] }),
            expect: vec![
                ("Properties", Expect::Map(0)),
                ("Source.SourceTypes", Expect::List(0)),
            ],
        },
        Row {
            case: "an artifact created with two properties and one source type",
            setup: vec![(
                "CreateArtifact",
                json!({
                    "ArtifactName": "artifact-filled",
                    "ArtifactType": "DataSet",
                    "Source": {
                        "SourceUri": "s3://bucket/filled",
                        "SourceTypes": [{ "SourceIdType": "Custom", "Value": "v1" }],
                    },
                    "Properties": { "left": "1", "right": "2" },
                }),
            )],
            read: "DescribeArtifact",
            request: |created| json!({ "ArtifactArn": created["ArtifactArn"] }),
            expect: vec![
                ("Properties", Expect::Map(2)),
                ("Source.SourceTypes", Expect::List(1)),
            ],
        },
        Row {
            case: "an action created with no properties",
            setup: vec![(
                "CreateAction",
                json!({
                    "ActionName": "action-bare",
                    "ActionType": "ModelDeployment",
                    "Source": { "SourceUri": "s3://bucket/action-bare" },
                }),
            )],
            read: "DescribeAction",
            request: |_| json!({ "ActionName": "action-bare" }),
            expect: vec![("Properties", Expect::Map(0))],
        },
        Row {
            case: "an action created with three properties",
            setup: vec![(
                "CreateAction",
                json!({
                    "ActionName": "action-filled",
                    "ActionType": "ModelDeployment",
                    "Source": { "SourceUri": "s3://bucket/action-filled" },
                    "Properties": { "a": "1", "b": "2", "c": "3" },
                }),
            )],
            read: "DescribeAction",
            request: |_| json!({ "ActionName": "action-filled" }),
            expect: vec![("Properties", Expect::Map(3))],
        },
        Row {
            case: "a context created with no properties",
            setup: vec![(
                "CreateContext",
                json!({
                    "ContextName": "context-bare",
                    "ContextType": "Endpoint",
                    "Source": { "SourceUri": "s3://bucket/context-bare" },
                }),
            )],
            read: "DescribeContext",
            request: |_| json!({ "ContextName": "context-bare" }),
            expect: vec![("Properties", Expect::Map(0))],
        },
        Row {
            case: "a context created with one property",
            setup: vec![(
                "CreateContext",
                json!({
                    "ContextName": "context-filled",
                    "ContextType": "Endpoint",
                    "Source": { "SourceUri": "s3://bucket/context-filled" },
                    "Properties": { "only": "1" },
                }),
            )],
            read: "DescribeContext",
            request: |_| json!({ "ContextName": "context-filled" }),
            expect: vec![("Properties", Expect::Map(1))],
        },
        Row {
            case: "a trial component created with none of its collections",
            setup: vec![(
                "CreateTrialComponent",
                json!({ "TrialComponentName": "component-bare" }),
            )],
            read: "DescribeTrialComponent",
            request: |_| json!({ "TrialComponentName": "component-bare" }),
            expect: vec![
                ("Parameters", Expect::Map(0)),
                ("InputArtifacts", Expect::Map(0)),
                ("OutputArtifacts", Expect::Map(0)),
                ("Metrics", Expect::List(0)),
                ("Sources", Expect::List(0)),
                // Nothing produced this component, so there is no source JOB to name: a
                // member the resource holds no value for stays absent.
                ("Source", Expect::Absent),
            ],
        },
        Row {
            case: "a trial component created with one input and two output artifacts",
            setup: vec![(
                "CreateTrialComponent",
                json!({
                    "TrialComponentName": "component-filled",
                    "Parameters": { "epochs": { "NumberValue": 4.0 } },
                    "InputArtifacts": { "train": { "Value": "s3://bucket/train" } },
                    "OutputArtifacts": {
                        "model": { "Value": "s3://bucket/model" },
                        "log": { "Value": "s3://bucket/log" },
                    },
                }),
            )],
            read: "DescribeTrialComponent",
            request: |_| json!({ "TrialComponentName": "component-filled" }),
            expect: vec![
                ("Parameters", Expect::Map(1)),
                ("InputArtifacts", Expect::Map(1)),
                ("OutputArtifacts", Expect::Map(2)),
                ("Metrics", Expect::List(0)),
                ("Sources", Expect::List(0)),
            ],
        },
        Row {
            case: "an AI workload configuration created with no tags",
            setup: vec![(
                "CreateAIWorkloadConfig",
                json!({ "AIWorkloadConfigName": "workload-untagged" }),
            )],
            read: "DescribeAIWorkloadConfig",
            request: |_| json!({ "AIWorkloadConfigName": "workload-untagged" }),
            expect: vec![("Tags", Expect::List(0))],
        },
        Row {
            case: "an AI workload configuration created with two tags",
            setup: vec![(
                "CreateAIWorkloadConfig",
                json!({ "AIWorkloadConfigName": "workload-tagged", "Tags": tags(2) }),
            )],
            read: "DescribeAIWorkloadConfig",
            request: |_| json!({ "AIWorkloadConfigName": "workload-tagged" }),
            expect: vec![("Tags", Expect::List(2))],
        },
        Row {
            case: "an AI recommendation job created with no tags",
            setup: vec![
                (
                    "CreateAIWorkloadConfig",
                    json!({ "AIWorkloadConfigName": "workload-for-untagged-job" }),
                ),
                (
                    "CreateAIRecommendationJob",
                    recommendation_job(
                        "recommendation-untagged",
                        "workload-for-untagged-job",
                        json!({}),
                    ),
                ),
            ],
            read: "DescribeAIRecommendationJob",
            request: |_| json!({ "AIRecommendationJobName": "recommendation-untagged" }),
            expect: vec![
                ("Tags", Expect::List(0)),
                // Nothing analysed the model, so the job holds no recommendation — which
                // the list reports as the empty one.
                ("Recommendations", Expect::List(0)),
                // The run never started, so there is no instant to report for it.
                ("StartTime", Expect::Absent),
            ],
        },
        Row {
            case: "an AI recommendation job created with three tags",
            setup: vec![
                (
                    "CreateAIWorkloadConfig",
                    json!({ "AIWorkloadConfigName": "workload-for-tagged-job" }),
                ),
                (
                    "CreateAIRecommendationJob",
                    recommendation_job(
                        "recommendation-tagged",
                        "workload-for-tagged-job",
                        json!({ "Tags": tags(3) }),
                    ),
                ),
            ],
            read: "DescribeAIRecommendationJob",
            request: |_| json!({ "AIRecommendationJobName": "recommendation-tagged" }),
            expect: vec![
                ("Tags", Expect::List(3)),
                ("Recommendations", Expect::List(0)),
            ],
        },
        Row {
            case: "an AI benchmark job created with no tags",
            setup: vec![
                (
                    "CreateAIWorkloadConfig",
                    json!({ "AIWorkloadConfigName": "workload-for-benchmark" }),
                ),
                // `CreateAIBenchmarkJob` resolves its `BenchmarkTarget` against the
                // endpoint store, so the endpoint it names is stood up first.
                (
                    "CreateModel",
                    json!({
                        "ModelName": "bench-endpoint-model",
                        "PrimaryContainer": primary_container(
                            "000000000000.dkr.ecr.us-east-1.amazonaws.com/infer:1",
                        ),
                        "ExecutionRoleArn": ROLE_ARN,
                    }),
                ),
                (
                    "CreateEndpointConfig",
                    one_variant_config("bench-endpoint-config", "bench-endpoint-model"),
                ),
                (
                    "CreateEndpoint",
                    json!({
                        "EndpointName": "bench-endpoint",
                        "EndpointConfigName": "bench-endpoint-config",
                    }),
                ),
                (
                    "CreateAIBenchmarkJob",
                    json!({
                        "AIBenchmarkJobName": "benchmark-untagged",
                        "AIWorkloadConfigIdentifier": "workload-for-benchmark",
                        "BenchmarkTarget": { "Endpoint": { "Identifier": "bench-endpoint" } },
                        "OutputConfig": { "S3OutputLocation": "s3://bucket/benchmarks/" },
                        "RoleArn": ROLE_ARN,
                    }),
                ),
            ],
            read: "DescribeAIBenchmarkJob",
            request: |_| json!({ "AIBenchmarkJobName": "benchmark-untagged" }),
            expect: vec![("Tags", Expect::List(0))],
        },
    ];

    run(rows).await;
}

/// RULE: a `UserContext` a resource's reader reports is present whatever the service holds
/// for it — the EMPTY structure until a call has recorded an identity — because the member
/// is how the resource reports who acted on it.
///
/// The rows span the forms an identity member takes here: a versioned card nothing has
/// modified and one an update HAS modified, a lineage entity whose identities the service
/// records with no Studio members at all, and an experiment entity that records the calling
/// identity at create.
#[tokio::test]
async fn a_user_context_member_is_reported_empty_until_a_call_records_an_identity() {
    let rows = vec![
        Row {
            case: "a model card nothing has modified",
            setup: vec![(
                "CreateModelCard",
                json!({
                    "ModelCardName": "card-unmodified",
                    "ModelCardStatus": "Draft",
                    "Content": card_content("unmodified"),
                }),
            )],
            read: "DescribeModelCard",
            request: |_| json!({ "ModelCardName": "card-unmodified" }),
            expect: vec![
                // Nothing has modified the card, so the member reports the empty
                // structure rather than going missing.
                ("LastModifiedBy", Expect::Map(0)),
                // The create recorded who made it, so the same structure carries one.
                ("CreatedBy", Expect::Map(1)),
            ],
        },
        Row {
            case: "a model card an update has modified",
            setup: vec![
                (
                    "CreateModelCard",
                    json!({
                        "ModelCardName": "card-modified",
                        "ModelCardStatus": "Draft",
                        "Content": card_content("modified"),
                    }),
                ),
                (
                    "UpdateModelCard",
                    json!({
                        "ModelCardName": "card-modified",
                        "ModelCardStatus": "PendingReview",
                    }),
                ),
            ],
            read: "DescribeModelCard",
            request: |_| json!({ "ModelCardName": "card-modified" }),
            expect: vec![
                // The update recorded an identity, so the SAME member now carries one:
                // the empty structure above is the absence of a modification rather than
                // a fixed answer.
                ("LastModifiedBy", Expect::Map(1)),
                ("CreatedBy", Expect::Map(1)),
            ],
        },
        Row {
            case: "a lineage action's identities",
            setup: vec![(
                "CreateAction",
                json!({
                    "ActionName": "action-identities",
                    "ActionType": "ModelDeployment",
                    "Source": { "SourceUri": "s3://bucket/action-identities" },
                }),
            )],
            read: "DescribeAction",
            request: |_| json!({ "ActionName": "action-identities" }),
            expect: vec![
                ("CreatedBy", Expect::Map(0)),
                ("LastModifiedBy", Expect::Map(0)),
            ],
        },
        Row {
            case: "a trial nothing has modified since its create",
            setup: vec![
                (
                    "CreateExperiment",
                    json!({ "ExperimentName": "experiment-for-trial" }),
                ),
                (
                    "CreateTrial",
                    json!({
                        "TrialName": "trial-identities",
                        "ExperimentName": "experiment-for-trial",
                    }),
                ),
            ],
            read: "DescribeTrial",
            request: |_| json!({ "TrialName": "trial-identities" }),
            expect: vec![
                ("CreatedBy", Expect::Map(1)),
                ("LastModifiedBy", Expect::Map(1)),
            ],
        },
    ];

    run(rows).await;

    // The identity a recorded `UserContext` carries is the IAM identity the request signed
    // with, so the structures counted above are the caller's rather than padding.
    let emulator = Emulator::new();
    let case = "the identity a create records";
    emulator
        .ok(
            "CreateModelCard",
            &json!({
                "ModelCardName": "card-identity",
                "ModelCardStatus": "Draft",
                "Content": card_content("identity"),
            }),
        )
        .await;
    let described = emulator
        .ok("DescribeModelCard", &json!({ "ModelCardName": "card-identity" }))
        .await;
    let creator = member(&described, "CreatedBy.IamIdentity.Arn").and_then(Value::as_str);
    assert!(
        creator.is_some_and(|arn| arn.starts_with("arn:aws:iam::")),
        "case {case:?}: expected CreatedBy to name the calling IAM identity, got {creator:?}"
    );
}

/// RULE: a job reports WHY its run ended without a result — the reason the stop recorded
/// for a job the caller stopped, and the reason its own run ended with otherwise — so the
/// member is served from what happened to the job rather than left absent.
#[tokio::test]
async fn a_recommendation_job_reports_why_its_run_ended() {
    // Rows: a job the caller stops, and one left to run. The reason differs with what
    // happened to the job, so neither sentence is every job's answer.
    let workload = "workload-for-reasons";
    let stopped = "recommendation-stopped";
    let left = "recommendation-left";

    let case = "a job the caller stopped";
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": workload }),
        )
        .await;
    emulator
        .ok(
            "CreateAIRecommendationJob",
            &recommendation_job(stopped, workload, json!({})),
        )
        .await;
    emulator
        .ok(
            "StopAIRecommendationJob",
            &json!({ "AIRecommendationJobName": stopped }),
        )
        .await;
    let describe_stopped = json!({ "AIRecommendationJobName": stopped });
    let stopping = emulator
        .ok("DescribeAIRecommendationJob", &describe_stopped)
        .await;
    assert_eq!(
        str_field(&stopping, "AIRecommendationJobStatus"),
        "Stopping",
        "case {case:?}: expected the transient Stopping status after the stop, got {stopping:?}"
    );
    assert_eq!(
        str_field(&stopping, "FailureReason"),
        STOPPED_BY_USER,
        "case {case:?}: expected the stop's reason while the job is Stopping, got {stopping:?}"
    );

    tokio::time::sleep(PAST_STOP_DUE).await;
    let settled = emulator
        .ok("DescribeAIRecommendationJob", &describe_stopped)
        .await;
    assert_eq!(
        str_field(&settled, "AIRecommendationJobStatus"),
        "Stopped",
        "case {case:?}: expected the job the caller stopped to end in Stopped, got {settled:?}"
    );
    assert_eq!(
        str_field(&settled, "FailureReason"),
        STOPPED_BY_USER,
        "case {case:?}: expected the stop's reason beside the terminal status, got {settled:?}"
    );
    // The same read reports the collections the job holds, whatever happened to it.
    check(case, &settled, "Recommendations", Expect::List(0));
    check(case, &settled, "Tags", Expect::List(0));

    let case = "a job left to run";
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": workload }),
        )
        .await;
    emulator
        .ok(
            "CreateAIRecommendationJob",
            &recommendation_job(left, workload, json!({})),
        )
        .await;
    tokio::time::sleep(PAST_JOB_END_DUE).await;
    let ended = emulator
        .ok(
            "DescribeAIRecommendationJob",
            &json!({ "AIRecommendationJobName": left }),
        )
        .await;
    assert_eq!(
        str_field(&ended, "AIRecommendationJobStatus"),
        "Failed",
        "case {case:?}: expected a job nobody stopped to end in Failed, got {ended:?}"
    );
    let reason = str_field(&ended, "FailureReason");
    assert!(
        !reason.is_empty() && reason != STOPPED_BY_USER,
        "case {case:?}: expected the reason the job's OWN run ended with rather than the \
         stop's sentence, got {reason:?}"
    );
    check(case, &ended, "Recommendations", Expect::List(0));
}
