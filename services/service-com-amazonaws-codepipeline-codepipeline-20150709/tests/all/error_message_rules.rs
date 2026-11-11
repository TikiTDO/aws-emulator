// SPDX-License-Identifier: CC0-1.0

//! How CodePipeline WORDS a refusal whose text it builds from the request, end to end
//! over the wire the service speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! Each rule is asserted over a SET of inputs, the rows differing along what the rule
//! generalises over (the identifier's alphabet and length, the version asked for, the
//! execution the id belongs to, the token presented, the resource identity, the count of
//! tags) — and over every operation the rule reaches, because each of these messages
//! comes from ONE formatter and two operations may not disagree about it:
//!
//! * a pipeline-VERSION lookup that found no such version names the account, the
//!   pipeline name and the version asked for;
//! * an execution lookup that found no such execution names the execution id and the
//!   pipeline it was looked up in;
//! * a stage lookup that found no such stage names the pipeline and the stage;
//! * a refused `nextToken` quotes the rejected cursor back, identically from every
//!   paginated listing;
//! * a tag-limit refusal names the RESOURCE the limit was exceeded for and not the
//!   count.
//!
//! The sibling suite `validation_and_messages.rs` pins the `PipelineNameInUseException`
//! text and the constraint-violation envelope, and `pipeline_not_found_wording.rs` pins
//! the `PipelineNotFoundException` text, which is NOT one formatter: CodePipeline words
//! it per API plane.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_TAGS_PER_RESOURCE;

use crate::pipeline_lifecycle::{build_stage, source_stage, Emulator, ACCOUNT_ID, ROLE_ARN};

/// `NextToken` is `@length(min: 1, max: 2048)`
/// (`smithy_query(query="shape", shape="NextToken")`), so these are the longest and the
/// shortest cursor a caller can even send.
const MAX_NEXT_TOKEN_LENGTH: usize = 2048;

/// How a table row builds a request naming a pipeline and an execution id.
type ExecutionRequest = fn(&str, &str) -> Value;

/// How a table row builds a request naming a stage of the fixture pipeline.
type StageRequest = fn(&str) -> Value;

/// How a table row builds a listing request carrying a cursor (and, for the listing
/// that addresses a resource, that resource's ARN).
type ListingRequest = fn(&str, &str) -> Value;

/// The `(status, code, message)` of a refusal. The awsJson `__type` may be prefixed with
/// the model's namespace, so only the shape name is compared.
async fn refusal(emulator: &Emulator, operation: &str, body: &Value) -> (u16, String, String) {
    let (status, response) = emulator.call(operation, body).await;
    let code = response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let message = response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, message)
}

/// A declaration with a source stage and a build stage, under `name`.
fn two_stage_declaration(name: &str) -> Value {
    json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [source_stage("Source"), build_stage("Build", 1)],
    })
}

/// `count` tags, each with a distinct key.
fn tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"key": format!("k{index}"), "value": format!("v{index}")}))
            .collect(),
    )
}

/// RULE: a `GetPipeline` call for a version the pipeline never had is refused with text
/// built from the account the lookup was scoped to, the name it asked for and the
/// version it asked for.
///
/// The rows differ in the pipeline NAME's alphabet and length, in how many versions the
/// pipeline has, and in the version asked for (just past the newest, far past it, and
/// the largest an `i32` carries) — and every version that DOES exist is still read, so
/// what is pinned is the lookup and not a blanket refusal.
#[tokio::test]
async fn a_missing_pipeline_version_names_the_account_the_name_and_the_version() {
    let emulator = Emulator::new();

    // (case, pipeline name, how many UPDATES after the create)
    let pipelines: Vec<(&str, String, usize)> = vec![
        ("one version", "verhist".to_string(), 0),
        // The name's alphabet: every character `PipelineName`'s pattern allows besides
        // the alphanumerics (`smithy_query(query="operation", operation="GetPipeline")`
        // reports `^[A-Za-z0-9.@\-_]+$`).
        ("a punctuated name", "a.b@c-d_e".to_string(), 1),
        ("the longest name", "n".repeat(100), 3),
    ];

    for (case, name, updates) in pipelines {
        emulator
            .ok("CreatePipeline", &json!({"pipeline": two_stage_declaration(&name)}))
            .await;
        for update in 0..updates {
            let mut declaration = two_stage_declaration(&name);
            declaration["stages"][1]["actions"][0]["configuration"]["ProjectName"] =
                json!(format!("project-{update}"));
            emulator
                .ok("UpdatePipeline", &json!({"pipeline": declaration}))
                .await;
        }
        let newest = i32::try_from(updates).expect("a small update count") + 1;

        // Every version the pipeline HAS is readable, so the refusals below are about
        // the version asked for and nothing else.
        for version in 1..=newest {
            let read = emulator
                .ok("GetPipeline", &json!({"name": name, "version": version}))
                .await;
            assert_eq!(
                read["pipeline"]["version"],
                json!(version),
                "case {case:?}: version {version} must still be readable, got {read}"
            );
        }

        for version in [newest + 1, 99, i32::MAX] {
            let (status, code, message) = refusal(
                &emulator,
                "GetPipeline",
                &json!({"name": name, "version": version}),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "PipelineVersionNotFoundException"),
                "case {case:?} version {version}: expected the modelled version refusal, \
                 got {status} {code} {message:?}"
            );
            assert_eq!(
                message,
                format!(
                    "Account '{ACCOUNT_ID}' does not have a pipeline with name '{name}' and \
                     version '{version}'."
                ),
                "case {case:?} version {version}: the message is built from the account, the \
                 name and the version, got {message:?}"
            );
        }
    }

    // One past the modelled lower bound (`PipelineVersion` is `@range(min: 1)`) is the
    // boundary validator's refusal, not the lookup's.
    let (status, code, message) = refusal(
        &emulator,
        "GetPipeline",
        &json!({"name": "verhist", "version": 0}),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"version 0\": a version outside the modelled range is a constraint \
         violation, got {status} {code} {message:?}"
    );
}

/// RULE: an execution id that names no execution of the pipeline it was looked up in is
/// refused with text built from both request members — the id, and the pipeline name.
///
/// The rows differ in the shape of the id (an id nothing ever minted, and the id of a
/// REAL execution of ANOTHER pipeline, which is the "does not belong to the specified
/// pipeline" half of the same rule) and in the operation that resolves it, since
/// `GetPipelineExecution`, `ListActionExecutions` and `ListRuleExecutions` all raise it.
#[tokio::test]
async fn an_unknown_execution_names_the_id_and_the_pipeline_it_was_looked_up_in() {
    let emulator = Emulator::new();
    for name in ["left", "right"] {
        emulator
            .ok("CreatePipeline", &json!({"pipeline": two_stage_declaration(name)}))
            .await;
    }
    let started = emulator
        .ok("StartPipelineExecution", &json!({"name": "right"}))
        .await;
    let other_pipelines_execution = started["pipelineExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("an execution id, got {started}"))
        .to_string();

    // (case, the id "left" does not hold) — every value satisfies
    // `PipelineExecutionId`'s modelled pattern, so the boundary validator passes it
    // through to the lookup.
    let ids: Vec<(&str, String)> = vec![
        (
            "an id nothing minted",
            "d8e17ed2-0971-4d07-acf5-0ba41a52682a".to_string(),
        ),
        ("the all-zero id", "00000000-0000-0000-0000-000000000000".to_string()),
        ("the all-f id", "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string()),
        (
            "a real execution of another pipeline",
            other_pipelines_execution.clone(),
        ),
    ];

    // (case, the operation, how the request names the execution)
    let operations: Vec<(&str, ExecutionRequest)> = vec![
        ("GetPipelineExecution", |pipeline, id| {
            json!({"pipelineName": pipeline, "pipelineExecutionId": id})
        }),
        ("ListActionExecutions", |pipeline, id| {
            json!({"pipelineName": pipeline, "filter": {"pipelineExecutionId": id}})
        }),
        ("ListRuleExecutions", |pipeline, id| {
            json!({"pipelineName": pipeline, "filter": {"pipelineExecutionId": id}})
        }),
    ];

    for (id_case, id) in &ids {
        for (operation, build) in &operations {
            let (status, code, message) =
                refusal(&emulator, operation, &build("left", id)).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "PipelineExecutionNotFoundException"),
                "case {id_case:?} via {operation}: expected the modelled execution refusal, \
                 got {status} {code} {message:?}"
            );
            assert_eq!(
                message,
                format!(
                    "Pipeline Execution with id '{id}' does not exist in pipeline with name \
                     'left'"
                ),
                "case {id_case:?} via {operation}: the message is built from the id and the \
                 pipeline, got {message:?}"
            );
        }
    }

    // The same id IS resolved in the pipeline that holds it, so the rule is the lookup
    // and not a refusal of every id.
    let read = emulator
        .ok(
            "GetPipelineExecution",
            &json!({
                "pipelineName": "right",
                "pipelineExecutionId": other_pipelines_execution,
            }),
        )
        .await;
    assert_eq!(
        read["pipelineExecution"]["pipelineExecutionId"],
        json!(other_pipelines_execution),
        "the execution is readable in its own pipeline, got {read}"
    );
}

/// RULE: a stage name that the named pipeline's declaration does not carry is refused
/// with text built from the pipeline name and the stage name.
///
/// The rows differ in the stage name's alphabet and length and in the operation that
/// resolves the stage — `DisableStageTransition`, `EnableStageTransition`,
/// `PutActionRevision` and `PutApprovalResult` each raise `StageNotFoundException` from
/// the same lookup — and a stage the declaration DOES carry is still accepted. Which
/// SENTENCE the refusal carries belongs to the plane the call came in on: the approval
/// plane states a name it cannot resolve as what the enclosing resource does not include
/// (as it states an absent pipeline and an absent action), the declaration plane states
/// it as the lookup that found nothing.
#[tokio::test]
async fn an_unknown_stage_names_the_pipeline_and_the_stage() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": two_stage_declaration("staged")}))
        .await;

    // Every name satisfies `StageName`'s pattern and length (1..100), so each reaches
    // the lookup rather than the boundary validator.
    let stage_names: Vec<(&str, String)> = vec![
        ("a plain absent name", "NoSuchStage".to_string()),
        ("a punctuated absent name", "No.Such@Stage-1_x".to_string()),
        ("a one-character absent name", "S".to_string()),
        ("the longest absent name", "z".repeat(100)),
    ];

    // (case, the operation, the request naming `pipelineName` and `stageName`)
    let operations: Vec<(&str, StageRequest)> = vec![
        ("DisableStageTransition", |stage| {
            json!({
                "pipelineName": "staged",
                "stageName": stage,
                "transitionType": "Inbound",
                "reason": "maintenance window",
            })
        }),
        ("EnableStageTransition", |stage| {
            json!({
                "pipelineName": "staged",
                "stageName": stage,
                "transitionType": "Outbound",
            })
        }),
        ("PutActionRevision", |stage| {
            json!({
                "pipelineName": "staged",
                "stageName": stage,
                "actionName": "Src",
                "actionRevision": {
                    "revisionId": "rev-1",
                    "revisionChangeId": "change-1",
                    "created": 1_700_000_000,
                },
            })
        }),
        ("PutApprovalResult", |stage| {
            json!({
                "pipelineName": "staged",
                "stageName": stage,
                "actionName": "Src",
                "result": {"summary": "looks good", "status": "Approved"},
                "token": "11111111-2222-4333-8444-555555555555",
            })
        }),
    ];

    for (stage_case, stage) in &stage_names {
        for (operation, build) in &operations {
            let (status, code, message) = refusal(&emulator, operation, &build(stage)).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "StageNotFoundException"),
                "case {stage_case:?} via {operation}: expected the modelled stage refusal, \
                 got {status} {code} {message:?}"
            );
            let expected = match *operation {
                "PutApprovalResult" => format!(
                    "staged does not include a stage named {stage}. Make sure you are using \
                     the correct name for the stage, and then try again."
                ),
                _ => format!("Pipeline 'staged' does not have a stage with name '{stage}'"),
            };
            assert_eq!(
                message, expected,
                "case {stage_case:?} via {operation}: the message is built from the pipeline \
                 and the stage, got {message:?}"
            );
        }
    }

    // A stage the declaration carries is accepted, so the rule is the lookup.
    emulator
        .ok(
            "DisableStageTransition",
            &json!({
                "pipelineName": "staged",
                "stageName": "Build",
                "transitionType": "Inbound",
                "reason": "maintenance window",
            }),
        )
        .await;
}

/// RULE: a `nextToken` no listing issued is refused with text that QUOTES the rejected
/// cursor back, and every paginated operation of this service words it identically.
///
/// The rows differ in the token (not base64 at all, base64 of text carrying no cursor
/// kind, a cursor of another listing's kind, and the shortest and longest token
/// `NextToken`'s length allows) and in the listing presented with it.
#[tokio::test]
async fn every_paginated_listing_words_an_invalid_next_token_the_same_way() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": two_stage_declaration("paged")}))
        .await;
    let arn = format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:paged");

    let tokens: Vec<(&str, String)> = vec![
        ("not base64 at all", "not-a-real-token==".to_string()),
        ("base64 of text with no cursor kind", "ZGVtbw==".to_string()),
        (
            "base64 of a cursor kind no listing of this service issues",
            "a2luZDpkZW1v".to_string(),
        ),
        ("the shortest token the model allows", "x".to_string()),
        (
            "the longest token the model allows",
            "t".repeat(MAX_NEXT_TOKEN_LENGTH),
        ),
    ];

    // (the operation, the request carrying the token) — every paginated operation this
    // service implements, each with whatever else its input requires. `ListWebhooks`
    // names its member `NextToken`, the others `nextToken`
    // (`smithy_query(query="operation", operation="ListWebhooks")`).
    let listings: Vec<(&str, ListingRequest)> = vec![
        ("ListPipelines", |token, _arn| json!({"nextToken": token})),
        ("ListWebhooks", |token, _arn| json!({"NextToken": token})),
        ("ListActionTypes", |token, _arn| json!({"nextToken": token})),
        ("ListTagsForResource", |token, arn| {
            json!({"resourceArn": arn, "nextToken": token})
        }),
        ("ListPipelineExecutions", |token, _arn| {
            json!({"pipelineName": "paged", "nextToken": token})
        }),
        ("ListActionExecutions", |token, _arn| {
            json!({"pipelineName": "paged", "nextToken": token})
        }),
        ("ListRuleExecutions", |token, _arn| {
            json!({"pipelineName": "paged", "nextToken": token})
        }),
    ];

    for (token_case, token) in &tokens {
        let expected = format!(
            "The nextToken value '{token}' is not valid. Specify a valid nextToken value, and \
             then try again."
        );
        for (operation, build) in &listings {
            let (status, code, message) =
                refusal(&emulator, operation, &build(token, &arn)).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidNextTokenException"),
                "case {token_case:?} via {operation}: expected the modelled cursor refusal, \
                 got {status} {code} {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {token_case:?} via {operation}: every listing words the rule the same \
                 way and quotes the rejected token, got {message:?}"
            );
        }
    }
}

/// RULE: a tag-limit refusal names the RESOURCE the limit was exceeded for, built from
/// the identity the request carries — never the tag count.
///
/// `CreateCustomActionType` is the row AWS's own text is known for, so its message is
/// compared whole; the rows differ in the action type's category, provider and version
/// and in how far past the bound the request is. The bound itself is asserted too: the
/// 50-tag request is accepted and the refused request registers nothing.
#[tokio::test]
async fn the_tag_limit_refusal_names_the_custom_action_type_it_was_exceeded_for() {
    let emulator = Emulator::new();

    let action_type = |category: &str, provider: &str, version: &str, tag_count: usize| {
        json!({
            "category": category,
            "provider": provider,
            "version": version,
            "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 1},
            "tags": tags(tag_count),
        })
    };

    // (case, category, provider, version, tag count) — the categories are the model's
    // own `ActionCategory` values (`smithy_query(query="shape",
    // shape="ActionCategory")`), including ones the recorded case did not carry.
    let rows: Vec<(&str, &str, &str, &str, usize)> = vec![
        (
            "one tag past the bound",
            "Build",
            "MyBuilder",
            "1",
            MAX_TAGS_PER_RESOURCE + 1,
        ),
        (
            "far past the bound",
            "Test",
            "MyTester",
            "2",
            4 * MAX_TAGS_PER_RESOURCE,
        ),
        (
            "a punctuated version past the bound",
            "Compute",
            "My_Runner-X",
            "v1_2-3",
            MAX_TAGS_PER_RESOURCE + 7,
        ),
    ];

    for (case, category, provider, version, tag_count) in rows {
        let request = action_type(category, provider, version, tag_count);
        let (status, code, message) =
            refusal(&emulator, "CreateCustomActionType", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "TooManyTagsException"),
            "case {case:?}: expected the modelled tag-limit refusal, got {status} {code} \
             {message:?}"
        );
        assert_eq!(
            message,
            format!(
                "Tag limit exceeded for custom action type with category=[{category}], \
                 provider=[{provider}] and version=[{version}]."
            ),
            "case {case:?}: the message names the action type the limit was exceeded for, \
             got {message:?}"
        );
        assert!(
            !message.contains(&tag_count.to_string()),
            "case {case:?}: the message does not report the count, got {message:?}"
        );

        // The refused request registered nothing: the identity it named is not in the
        // catalog the listing reports.
        let listed = emulator.ok("ListActionTypes", &json!({})).await;
        let registered = listed["actionTypes"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a list of action types, got {listed}"))
            .iter()
            .any(|action_type| {
                action_type["id"]["provider"] == json!(provider)
                    && action_type["id"]["version"] == json!(version)
            });
        assert!(
            !registered,
            "case {case:?}: a refused create must register nothing, got {listed}"
        );

        // The same request AT the bound is accepted, so what is pinned is the bound.
        let at_bound = action_type(category, provider, version, MAX_TAGS_PER_RESOURCE);
        let created = emulator.ok("CreateCustomActionType", &at_bound).await;
        assert_eq!(
            created["tags"].as_array().map(Vec::len),
            Some(MAX_TAGS_PER_RESOURCE),
            "case {case:?}: a request at the bound is accepted with all its tags, got \
             {created}"
        );
    }
}

/// RULE, continued: every other operation that applies tags words the same refusal the
/// same way — the resource identity ITS request carries, and no count.
///
/// The rows are the three remaining tagging surfaces: a pipeline named by its
/// declaration, a webhook named by its definition, and the resource an ARN addresses.
#[tokio::test]
async fn the_tag_limit_refusal_names_the_resource_for_every_tagging_operation() {
    let emulator = Emulator::new();
    emulator
        .ok("CreatePipeline", &json!({"pipeline": two_stage_declaration("tagged")}))
        .await;
    let arn = format!("arn:aws:codepipeline:us-east-1:{ACCOUNT_ID}:tagged");

    let over = MAX_TAGS_PER_RESOURCE + 1;
    // (case, operation, request, the identity the message must name)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a pipeline created with too many tags",
            "CreatePipeline",
            json!({
                "pipeline": two_stage_declaration("over-tagged"),
                "tags": tags(over),
            }),
            "over-tagged".to_string(),
        ),
        (
            "a webhook defined with too many tags",
            "PutWebhook",
            json!({
                "webhook": {
                    "name": "over-tagged-hook",
                    "targetPipeline": "tagged",
                    "targetAction": "Src",
                    "filters": [{"jsonPath": "$.ref"}],
                    "authentication": "UNAUTHENTICATED",
                    "authenticationConfiguration": {},
                },
                "tags": tags(over),
            }),
            "over-tagged-hook".to_string(),
        ),
        (
            "a resource tagged past the bound by ARN",
            "TagResource",
            json!({"resourceArn": arn, "tags": tags(over)}),
            arn.clone(),
        ),
    ];

    for (case, operation, request, identity) in rows {
        let (status, code, message) = refusal(&emulator, operation, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "TooManyTagsException"),
            "case {case:?}: expected the modelled tag-limit refusal, got {status} {code} \
             {message:?}"
        );
        assert!(
            message.starts_with("Tag limit exceeded for "),
            "case {case:?}: the refusal states the limit was exceeded for a resource, got \
             {message:?}"
        );
        assert!(
            message.contains(&identity),
            "case {case:?}: the message names the resource ({identity}), got {message:?}"
        );
        assert!(
            !message.contains(&over.to_string()),
            "case {case:?}: the message does not report the count, got {message:?}"
        );
    }

    // Nothing the refused calls addressed was created or tagged.
    let pipelines = emulator.ok("ListPipelines", &json!({})).await;
    assert!(
        !pipelines["pipelines"]
            .as_array()
            .unwrap_or_else(|| panic!("a list of pipelines, got {pipelines}"))
            .iter()
            .any(|pipeline| pipeline["name"] == json!("over-tagged")),
        "a refused CreatePipeline must store no pipeline, got {pipelines}"
    );
    let webhooks = emulator.ok("ListWebhooks", &json!({})).await;
    assert_eq!(
        webhooks["webhooks"],
        json!([]),
        "a refused PutWebhook must store no webhook, got {webhooks}"
    );
    let held = emulator
        .ok("ListTagsForResource", &json!({"resourceArn": arn}))
        .await;
    assert!(
        held["tags"]
            .as_array()
            .is_none_or(|tags| tags.is_empty()),
        "a refused TagResource must write no tag, got {held}"
    );
}
