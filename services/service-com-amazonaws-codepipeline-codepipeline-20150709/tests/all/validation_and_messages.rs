// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what CodePipeline REFUSES, and how it words each refusal.
//!
//! The rules under test, each over a SET of inputs:
//!
//! * `CreatePipeline` refuses a declaration that is structurally impossible — an
//!   `inputArtifacts` entry no earlier action outputs, a first stage that is not the
//!   source stage, a `Source` action outside it, a `roleArn` IAM holds no role for, and
//!   the V2-only members on a declaration that states `pipelineType` `V1` — with the
//!   error shape the model declares for a malformed structure;
//! * `PipelineNameInUseException` carries the text its rule builds from the REQUEST and
//!   the caller's account, so the account the name collided in is reported, and so does
//!   the pipeline-definition plane's `PipelineNotFoundException` (the OTHER planes word
//!   that error their own way — `pipeline_not_found_wording.rs` pins all six);
//! * a declaration whose first stage is not made up exclusively of `Source` actions is
//!   refused with the FIXED sentence that defect class carries, with nothing of the
//!   request interpolated into it, by every operation that takes a declaration;
//! * a constraint-violation message names the member path and the constraint and
//!   NOTHING of the rejected value, states a `@pattern` without the `^`/`$` anchors the
//!   model spells it with, and reports a constraint of a map's ENTRY shape against the
//!   map member with that shape's whole constraint set.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The canonical emulator account id (`emulator_server::EMULATOR_ACCOUNT_ID`), which is
/// the account every refusal here is scoped to.
const ACCOUNT_ID: &str = "000000000000";

/// The role every accepted declaration names, created in IAM by [`Emulator::role`].
const ROLE_NAME: &str = "cp-execution-role";

/// A trust policy with a single statement — the form `CreateRole`'s own modelled example
/// carries, and all it asks of one.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"codepipeline.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The `@pattern` of `PipelineName` and of `WebhookName`, without the anchors the model
/// spells them with (`smithy_query(query="shape", shape="PipelineName,WebhookName")`
/// reports `^[A-Za-z0-9.@\-_]+$` for both).
const NAME_PATTERN: &str = r"[A-Za-z0-9.@\-_]+";

/// `ActionConfigurationValue`'s `@length` bounds
/// (`smithy_query(query="shape", shape="ActionConfigurationMap")`: value
/// `ActionConfigurationValue length 1..1000`).
const CONFIGURATION_VALUE_BOUNDS: (i64, i64) = (1, 1000);

/// One emulator with CodePipeline and IAM mounted on one engine: `CreatePipeline` checks
/// the declaration's role in IAM.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call one CodePipeline operation.
    async fn cp(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/codepipeline/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";
        let request = http::Request::builder()
            .method("POST")
            .uri("https://codepipeline.us-east-1.amazonaws.com/")
            .header("host", "codepipeline.us-east-1.amazonaws.com")
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!("{operation}: response body is not JSON ({error}): {body:?}")
            })
        };
        (status, parsed)
    }

    /// The `(status, code, message)` of a refusal.
    async fn refusal(&self, operation: &str, body: &Value) -> (u16, String, String) {
        let (status, response) = self.cp(operation, body).await;
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

    /// Create the role every accepted declaration names, over IAM's awsQuery wire.
    async fn role(&self) {
        let body = format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={ROLE_NAME}\
             &AssumeRolePolicyDocument={}",
            form_encode(TRUST_POLICY)
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(bytes::Bytes::from(body))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        assert_eq!(
            status,
            200,
            "the fixture role must be created, got {status}: {}",
            String::from_utf8_lossy(&response.into_body())
        );
    }
}

/// Percent-encode one form value the way an awsQuery body carries it — the shared
/// harness's rule (`pipeline_lifecycle::form_encode`), not a second copy.
fn form_encode(value: &str) -> String {
    crate::pipeline_lifecycle::form_encode(value)
}

/// The ARN of the role IAM holds.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/{ROLE_NAME}")
}

/// A source stage, the shape every valid declaration starts with.
fn source_stage(output: &str) -> Value {
    json!({
        "name": "Source",
        "actions": [{
            "name": "Src",
            "actionTypeId": {
                "category": "Source", "owner": "AWS", "provider": "S3", "version": "1"
            },
            "configuration": {"S3Bucket": "bucket", "S3ObjectKey": "source.zip"},
            "outputArtifacts": [{"name": output}],
        }],
    })
}

/// A deploy stage consuming `input`.
fn deploy_stage(name: &str, input: &str) -> Value {
    json!({
        "name": name,
        "actions": [{
            "name": "Dep",
            "actionTypeId": {
                "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1"
            },
            "configuration": {"BucketName": "out", "ObjectKey": "k", "Extract": "false"},
            "inputArtifacts": [{"name": input}],
        }],
    })
}

/// A stage carrying one action outside the `Source` category and no artifact members.
///
/// What a declaration has to carry beside its source stage: a pipeline acts on the
/// revision it fetched, and carries at least two stages
/// (`state::validate_declaration`). A row whose own subject is the stage list builds
/// its stages itself.
fn acting_stage(name: &str) -> Value {
    json!({"name": name, "actions": [categorised_action("Act", "Build", "CodeBuild", 1)]})
}

/// A declaration built from its stages, with the role IAM holds.
fn declaration(name: &str, stages: Value) -> Value {
    json!({"pipeline": {
        "name": name,
        "roleArn": role_arn(),
        "artifactStore": {"type": "S3", "location": "artifacts"},
        "stages": stages,
    }})
}

#[tokio::test]
async fn create_pipeline_refuses_a_structurally_impossible_declaration() {
    let emulator = Emulator::new();
    emulator.role().await;

    // One row per structural rule, plus the declaration each rule's negative case is a
    // variation of: what the rows differ in is the SHAPE of the declaration.
    let rows: Vec<(&str, Value)> = vec![
        (
            "an input artifact no earlier action outputs",
            declaration(
                "dangling-input",
                json!([source_stage("SourceArtifact"), deploy_stage("Deploy", "NoSuchArtifact")]),
            ),
        ),
        (
            "a source action outside the first stage",
            declaration(
                "source-not-first",
                json!([
                    {
                        "name": "Gate",
                        "actions": [{
                            "name": "Approve",
                            "actionTypeId": {
                                "category": "Approval", "owner": "AWS",
                                "provider": "Manual", "version": "1",
                            },
                        }],
                    },
                    source_stage("SourceArtifact"),
                ]),
            ),
        ),
        (
            "a non-source action in the first stage",
            declaration(
                "mixed-first-stage",
                json!([{
                    "name": "Ingest",
                    "actions": [
                        {
                            "name": "Src",
                            "actionTypeId": {
                                "category": "Source", "owner": "AWS",
                                "provider": "S3", "version": "1",
                            },
                            "outputArtifacts": [{"name": "SourceArtifact"}],
                        },
                        {
                            "name": "Approve",
                            "actionTypeId": {
                                "category": "Approval", "owner": "AWS",
                                "provider": "Manual", "version": "1",
                            },
                            "runOrder": 2,
                        },
                    ],
                }, acting_stage("Build")]),
            ),
        ),
        (
            "one output artifact name declared twice",
            declaration(
                "duplicate-output",
                json!([
                    source_stage("SourceArtifact"),
                    {
                        "name": "Build",
                        "actions": [{
                            "name": "Again",
                            "actionTypeId": {
                                "category": "Build", "owner": "AWS",
                                "provider": "CodeBuild", "version": "1",
                            },
                            "inputArtifacts": [{"name": "SourceArtifact"}],
                            "outputArtifacts": [{"name": "SourceArtifact"}],
                        }],
                    },
                ]),
            ),
        ),
        (
            "no stage at all",
            declaration("no-stages", json!([])),
        ),
    ];

    for (case, body) in rows {
        let (status, code, message) = emulator.refusal("CreatePipeline", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidStructureException"),
            "case {case:?}: expected the modelled structure refusal, got {status} {code} \
             {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: the refusal says what is wrong, got {message:?}"
        );
    }

    // A V1 declaration cannot carry the V2-only members, one row each.
    for (case, member, value) in [
        (
            "variables on a V1 pipeline",
            "variables",
            json!([{"name": "Flavor", "defaultValue": "vanilla"}]),
        ),
        (
            "triggers on a V1 pipeline",
            "triggers",
            json!([{
                "providerType": "CodeStarSourceConnection",
                "gitConfiguration": {"sourceActionName": "Src"},
            }]),
        ),
    ] {
        let mut body = declaration(
            "v1-pipeline",
            json!([source_stage("SourceArtifact"), acting_stage("Build")]),
        );
        body["pipeline"]["pipelineType"] = json!("V1");
        body["pipeline"][member] = value;
        let (status, code, message) = emulator.refusal("CreatePipeline", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidStructureException"),
            "case {case:?}: expected the modelled structure refusal, got {status} {code} \
             {message:?}"
        );
        assert!(
            message.contains(member),
            "case {case:?}: the refusal names the member, got {message:?}"
        );
    }

    // A roleArn IAM holds no role for is refused the same way; the SAME declaration with
    // the role that does exist is accepted, so the rule is the role's existence and not
    // the shape of the ARN.
    let mut absent_role = declaration(
        "absent-role",
        json!([source_stage("SourceArtifact"), acting_stage("Build")]),
    );
    absent_role["pipeline"]["roleArn"] =
        json!(format!("arn:aws:iam::{ACCOUNT_ID}:role/role-nobody-created"));
    let (status, code, message) = emulator.refusal("CreatePipeline", &absent_role).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidStructureException"),
        "a roleArn naming no role is refused, got {status} {code} {message:?}"
    );
    let absent_role_message = message.clone();

    // The role is settled BEFORE the declaration is walked: the service decides whether
    // it could run this pipeline's actions at all before it judges what those actions
    // are, so a declaration that is ALSO malformed in its own right — here, below the
    // stage floor — is still refused for the role, with the role's own message.
    let mut absent_role_and_malformed =
        declaration("absent-role-malformed", json!([source_stage("SourceArtifact")]));
    absent_role_and_malformed["pipeline"]["roleArn"] =
        json!(format!("arn:aws:iam::{ACCOUNT_ID}:role/role-nobody-created"));
    let (status, code, message) = emulator
        .refusal("CreatePipeline", &absent_role_and_malformed)
        .await;
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (
            400,
            "InvalidStructureException",
            absent_role_message.as_str()
        ),
        "the role is resolved before the declaration is walked, got {status} {code} \
         {message:?}"
    );

    let accepted = declaration(
        "valid-pipeline",
        json!([source_stage("SourceArtifact"), acting_stage("Build")]),
    );
    let (status, answer) = emulator.cp("CreatePipeline", &accepted).await;
    assert_eq!(
        status, 200,
        "a structurally valid declaration naming a role that exists is accepted, got \
         {status} with {answer}"
    );
}

#[tokio::test]
async fn a_declaration_past_a_quota_is_refused_before_it_is_walked() {
    let emulator = Emulator::new();
    emulator.role().await;
    let stages = service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_STAGES_PER_PIPELINE;
    let actions_per_stage =
        service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_ACTIONS_PER_STAGE;
    let actions_per_pipeline =
        service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_ACTIONS_PER_PIPELINE;
    let configuration_entries = service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_CONFIGURATION_ENTRIES_PER_ACTION;

    // A deploy stage carrying `count` actions, each consuming the source artifact.
    let wide_stage = |name: &str, count: usize| {
        json!({
            "name": name,
            "actions": (0..count).map(|index| json!({
                "name": format!("Dep{index}"),
                "actionTypeId": {
                    "category": "Deploy", "owner": "AWS", "provider": "S3", "version": "1"
                },
                "configuration": {"BucketName": "out", "ObjectKey": format!("k{index}")},
                "inputArtifacts": [{"name": "SourceArtifact"}],
            })).collect::<Vec<_>>(),
        })
    };

    // One row per quota: the declaration is one past the bound, and the row under the
    // bound is accepted, so what is pinned is the bound itself and not a blanket refusal.
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "one stage past the stage limit",
            Value::Array(
                std::iter::once(source_stage("SourceArtifact"))
                    .chain((1..=stages).map(|index| wide_stage(&format!("S{index}"), 1)))
                    .collect(),
            ),
            false,
        ),
        (
            "at the stage limit",
            Value::Array(
                std::iter::once(source_stage("SourceArtifact"))
                    .chain((1..stages).map(|index| wide_stage(&format!("S{index}"), 1)))
                    .collect(),
            ),
            true,
        ),
        (
            "one action past the per-stage limit",
            json!([
                source_stage("SourceArtifact"),
                wide_stage("Wide", actions_per_stage + 1),
            ]),
            false,
        ),
        (
            "one action past the per-pipeline limit",
            Value::Array(
                std::iter::once(source_stage("SourceArtifact"))
                    .chain(
                        (0..3).map(|index| {
                            wide_stage(&format!("Many{index}"), actions_per_pipeline / 2)
                        }),
                    )
                    .collect(),
            ),
            false,
        ),
    ];

    for (case, stages, accepted) in rows {
        let mut body = declaration(&format!("quota-{}", case.replace(' ', "-")), stages);
        if accepted {
            let (status, answer) = emulator.cp("CreatePipeline", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a declaration inside the quotas is accepted, got {status} \
                 with {answer}"
            );
            continue;
        }
        let (status, code, message) = emulator.refusal("CreatePipeline", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "LimitExceededException"),
            "case {case:?}: a declaration past a quota is refused, got {status} {code} \
             {message:?}"
        );
        // The refusal says which quota, so the caller can act on it.
        assert!(
            message.contains("limit of"),
            "case {case:?}: the refusal names the limit, got {message:?}"
        );
        body["pipeline"]["name"] = json!("quota-unused");
    }

    // An action's configuration is bounded too.
    let mut body = declaration(
        "quota-configuration",
        json!([source_stage("SourceArtifact"), deploy_stage("Deploy", "SourceArtifact")]),
    );
    let mut configuration = serde_json::Map::new();
    for index in 0..=configuration_entries {
        configuration.insert(format!("K{index}"), json!("v"));
    }
    body["pipeline"]["stages"][1]["actions"][0]["configuration"] = Value::Object(configuration);
    let (status, code, message) = emulator.refusal("CreatePipeline", &body).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "an action past the configuration-entry limit is refused, got {status} {code} \
         {message:?}"
    );

    // So are an action's artifact lists, on each of the two members and at the bound:
    // the walk that pairs produced names with consumed ones is sized by these counts.
    let artifacts = service_com_amazonaws_codepipeline_codepipeline_20150709::state::MAX_ARTIFACTS_PER_ACTION;
    for (case, count, accepted) in [
        ("at the per-action artifact limit", artifacts, true),
        ("one past the per-action artifact limit", artifacts + 1, false),
    ] {
        let outputs: Vec<Value> = (0..count)
            .map(|index| json!({"name": format!("{}Out{index}", case.len())}))
            .collect();
        let body = declaration(
            &format!("quota-artifacts-{count}"),
            json!([
                {
                    "name": "Source",
                    "actions": [{
                        "name": "Src",
                        // A `Custom`-owned source action type: the bound this row is
                        // about is the EMULATOR's own per-action artifact cap
                        // (`MAX_ARTIFACTS_PER_ACTION`), and an action type the service
                        // publishes would be refused ahead of it by its own published
                        // artifact maximum (`state::validate_action_type`).
                        "actionTypeId": {
                            "category": "Source", "owner": "Custom", "provider": "MySource",
                            "version": "1"
                        },
                        "configuration": {"S3Bucket": "b", "S3ObjectKey": "k"},
                        "outputArtifacts": outputs,
                    }],
                },
                acting_stage("Build"),
            ]),
        );
        if accepted {
            let (status, answer) = emulator.cp("CreatePipeline", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a declaration inside the artifact limit is accepted, got \
                 {status} with {answer}"
            );
            continue;
        }
        let (status, code, message) = emulator.refusal("CreatePipeline", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "LimitExceededException"),
            "case {case:?}: expected a 400 LimitExceededException, got {status} {code} \
             {message:?}"
        );
        assert!(
            message.contains("outputArtifacts") && message.contains("limit of"),
            "case {case:?}: the refusal names the member and the limit, got {message:?}"
        );
    }
}

#[tokio::test]
async fn the_pipeline_name_refusals_carry_the_text_their_rule_builds() {
    let emulator = Emulator::new();
    emulator.role().await;

    // The rule generalises over the NAME, so the rows differ in the name's alphabet and
    // length rather than only in spelling.
    for name in ["dup", "a.b@c-d_e", &"n".repeat(100)] {
        let body = declaration(
            name,
            json!([source_stage("SourceArtifact"), acting_stage("Build")]),
        );
        let (status, answer) = emulator.cp("CreatePipeline", &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the first create must succeed, got {status} with {answer}"
        );
        let (status, code, message) = emulator.refusal("CreatePipeline", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineNameInUseException"),
            "case {name:?}: a second create under the same name is refused, got {status} \
             {code} {message:?}"
        );
        assert_eq!(
            message,
            format!("A pipeline with the name '{name}' already exists in account '{ACCOUNT_ID}'"),
            "case {name:?}: the message is built from the request and the account"
        );
    }

    // The pipeline-DEFINITION plane: the calls that read or write the declaration itself
    // all word a lookup that found nothing the same way. The planes that word it
    // otherwise are `pipeline_not_found_wording.rs`.
    for (operation, body) in [
        ("GetPipeline", json!({"name": "absent-pipeline"})),
        ("GetPipelineState", json!({"name": "absent-pipeline"})),
        (
            "UpdatePipeline",
            declaration(
                "absent-pipeline",
                json!([source_stage("SourceArtifact"), acting_stage("Build")]),
            ),
        ),
    ] {
        let (status, code, message) = emulator.refusal(operation, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "PipelineNotFoundException"),
            "case {operation:?}: expected the modelled not-found refusal, got {status} \
             {code} {message:?}"
        );
        assert_eq!(
            message,
            format!("Account '{ACCOUNT_ID}' does not have a pipeline with name 'absent-pipeline'"),
            "case {operation:?}: the message is built from the request and the account"
        );
    }
}

#[tokio::test]
async fn a_constraint_violation_names_the_member_and_the_constraint_only() {
    let emulator = Emulator::new();

    // One row per constrained member class: a `@pattern` on a top-level member, the same
    // pattern reached through a nested path, and a `@length` lower bound. Each row's
    // value is one a client could send, and none of it may appear in the message.
    let rows: Vec<(&str, &str, Value, String, &str)> = vec![
        (
            "a webhook name outside the pattern",
            "DeregisterWebhookWithThirdParty",
            json!({"webhookName": "bad name!"}),
            "webhookName".to_string(),
            "bad name!",
        ),
        (
            "a webhook name with a non-ASCII character",
            "RegisterWebhookWithThirdParty",
            json!({"webhookName": "wébhook"}),
            "webhookName".to_string(),
            "wébhook",
        ),
        (
            "a pipeline name with a non-ASCII character",
            "CreatePipeline",
            declaration(
                "pipe-ünïcode",
                json!([source_stage("SourceArtifact"), acting_stage("Build")]),
            ),
            "pipeline.name".to_string(),
            "pipe-ünïcode",
        ),
    ];

    for (case, operation, body, path, rejected) in rows {
        let (status, code, message) = emulator.refusal(operation, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the modelled validation refusal, got {status} {code} \
             {message:?}"
        );
        assert_eq!(
            message,
            format!(
                "1 validation error detected: Value at '{path}' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {NAME_PATTERN}"
            ),
            "case {case:?}: the message is the member path and the constraint"
        );
        assert!(
            !message.contains(rejected),
            "case {case:?}: the rejected value is not echoed back, got {message:?}"
        );
        assert!(
            !message.contains('^') && !message.contains("$"),
            "case {case:?}: the pattern carries no anchors, got {message:?}"
        );
    }

    // A `@length` violation states the bound, still without the value.
    let (status, code, message) = emulator
        .refusal("GetPipeline", &json!({"name": "n".repeat(101)}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "an over-long name is refused, got {status} {code} {message:?}"
    );
    assert_eq!(
        message,
        "1 validation error detected: Value at 'name' failed to satisfy constraint: \
         Member must have length less than or equal to 100",
        "the message states the bound and no value"
    );

    // An absent required member is reported as the null it was.
    let (status, code, message) = emulator
        .refusal("GetPipeline", &json!({}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "an absent required member is refused, got {status} {code} {message:?}"
    );
    assert_eq!(
        message,
        "1 validation error detected: Value null at 'name' failed to satisfy constraint: \
         Member must not be null",
        "an absent member is reported as null"
    );

    // An ENUM violation states the value set the member accepts, still without the
    // rejected value — `BlockerType` declares the single value `Schedule`
    // (`smithy_query(query="shape", shape="BlockerType")`), and a stage's `blockers`
    // reach it through a nested member path.
    let mut blockers = declaration(
        "blocker-pipeline",
        json!([source_stage("SourceArtifact"), deploy_stage("Deploy", "SourceArtifact")]),
    );
    blockers["pipeline"]["stages"][1]["blockers"] =
        json!([{"name": "Window", "type": "Unsupported"}]);
    let (status, code, message) = emulator.refusal("CreatePipeline", &blockers).await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "a blocker type outside the enum is refused, got {status} {code} {message:?}"
    );
    assert_eq!(
        message,
        "1 validation error detected: Value at \
         'pipeline.stages.2.member.blockers.1.member.type' failed to satisfy constraint: \
         Member must satisfy enum value set: [Schedule]",
        "the message names the member path and the value set, and not the value"
    );
    assert!(
        !message.contains("Unsupported"),
        "the rejected enum value is not echoed back, got {message:?}"
    );
}

#[tokio::test]
async fn a_map_entrys_constraint_is_reported_against_the_map_member() {
    let emulator = Emulator::new();
    let (min, max) = CONFIGURATION_VALUE_BOUNDS;

    // The rule generalises over WHICH entry of the map violates the constraint and over
    // where the map sits in the input, so the rows differ in the key (including a
    // one-character, a non-ASCII and an EMPTY key, each of which is request-controlled
    // text the message's path is built from), in the stage index and in the operation
    // carrying the declaration.
    let rows: Vec<(&str, &str, &str, usize)> = vec![
        ("an empty value under one key", "CreatePipeline", "CustomData", 0),
        ("an empty value under another key", "UpdatePipeline", "S3ObjectKey", 0),
        ("an empty value in a later stage", "UpdatePipeline", "Extract", 1),
        ("a one-character key", "CreatePipeline", "K", 0),
        ("a non-ASCII key", "CreatePipeline", "ünïcode", 0),
        ("an empty key", "CreatePipeline", "", 0),
    ];

    for (case, operation, key, stage_index) in rows {
        let mut stages = vec![source_stage("SourceArtifact"), deploy_stage("Deploy", "SourceArtifact")];
        stages[stage_index]["actions"][0]["configuration"][key] = json!("");
        let body = declaration("map-pipeline", Value::Array(stages));
        let (status, code, message) = emulator.refusal(operation, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the modelled validation refusal, got {status} {code} \
             {message:?}"
        );
        assert!(
            message.contains(&format!(
                "failed to satisfy constraint: Map value must satisfy constraint: [Member \
                 must have length less than or equal to {max}, Member must have length \
                 greater than or equal to {min}]"
            )),
            "case {case:?}: the requirement is the entry shape's whole constraint set, \
             got {message:?}"
        );
        assert!(
            message.contains("configuration' failed"),
            "case {case:?}: the path named is the map member, got {message:?}"
        );
        assert!(
            !message.contains(&format!("configuration.{key}")),
            "case {case:?}: the entry's own key is not the path AWS names, got {message:?}"
        );
    }
}

/// The FIXED sentence a declaration whose first stage is not made up exclusively of
/// source actions is refused with. A constant for this defect class: no stage name,
/// action name, category, count or index is interpolated into it.
const FIRST_STAGE_MESSAGE: &str = "Pipeline should start with a stage that only contains source actions";

/// One action with no artifact members: the category and provider are what the
/// first-stage law looks at.
fn categorised_action(name: &str, category: &str, provider: &str, run_order: i32) -> Value {
    json!({
        "name": name,
        "actionTypeId": {
            "category": category, "owner": "AWS", "provider": provider, "version": "1",
        },
        "runOrder": run_order,
    })
}

/// A source action with no artifact members, for a first stage under test.
fn bare_source_action(name: &str, run_order: i32) -> Value {
    json!({
        "name": name,
        "actionTypeId": {
            "category": "Source", "owner": "AWS", "provider": "S3", "version": "1",
        },
        "configuration": {"S3Bucket": "bucket", "S3ObjectKey": "source.zip"},
        "runOrder": run_order,
    })
}

/// A first stage that is not the pipeline's source stage is refused with the fixed
/// sentence of that defect class, whatever the declaration that violates the law looks
/// like — and nothing of the request is interpolated into it.
#[tokio::test]
async fn a_first_stage_that_is_not_the_source_stage_carries_the_fixed_message() {
    let emulator = Emulator::new();
    emulator.role().await;

    // The law generalises over the COMPOSITION of the first stage, so the rows differ in
    // how it is composed: no source action at all, a source action mixed with a
    // non-source one (in either order of `runOrder`), and several non-source actions.
    // `tokens` is the text each row puts in the request that an interpolating message
    // would leak.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "an approval stage first, the source stage second",
            json!([
                {"name": "Gate", "actions": [
                    categorised_action("Approve", "Approval", "Manual", 1),
                ]},
                source_stage("SourceArtifact"),
            ]),
            vec!["Gate", "Approve", "Approval", "Manual"],
        ),
        (
            "a build stage first, the source stage second",
            json!([
                {"name": "Prebuild", "actions": [
                    categorised_action("Compile", "Build", "CodeBuild", 1),
                ]},
                source_stage("SourceArtifact"),
            ]),
            vec!["Prebuild", "Compile", "Build", "CodeBuild"],
        ),
        (
            "a source action and a non-source action in the first stage",
            json!([
                {"name": "Ingest", "actions": [
                    bare_source_action("Src", 1),
                    categorised_action("Hold", "Approval", "Manual", 2),
                ]},
                acting_stage("Build"),
            ]),
            vec!["Ingest", "Hold", "Approval", "Manual"],
        ),
        (
            "a non-source action before the source action in the first stage",
            json!([
                {"name": "Ingest", "actions": [
                    categorised_action("Hold", "Invoke", "Lambda", 1),
                    bare_source_action("Src", 2),
                ]},
                acting_stage("Build"),
            ]),
            vec!["Ingest", "Hold", "Invoke", "Lambda"],
        ),
        (
            "two non-source actions in the first stage",
            json!([
                {"name": "Zulu", "actions": [
                    categorised_action("Hold", "Approval", "Manual", 1),
                    categorised_action("Probe", "Test", "CodeBuild", 2),
                ]},
                source_stage("SourceArtifact"),
            ]),
            vec!["Zulu", "Hold", "Probe", "Test", "CodeBuild"],
        ),
        (
            "a first stage carrying no source action",
            json!([
                {"name": "Verify", "actions": [
                    categorised_action("Probe", "Test", "CodeBuild", 1),
                ]},
                acting_stage("Ship"),
            ]),
            vec!["Verify", "Probe", "Test", "CodeBuild"],
        ),
    ];

    for (index, (case, stages, tokens)) in rows.into_iter().enumerate() {
        let name = format!("first-stage-{index}");
        let body = declaration(&name, stages);
        let (status, code, message) = emulator.refusal("CreatePipeline", &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidStructureException"),
            "case {case:?}: expected the modelled structure refusal, got {status} {code} \
             {message:?}"
        );
        assert_eq!(
            message, FIRST_STAGE_MESSAGE,
            "case {case:?}: expected the fixed first-stage sentence, got {message:?}"
        );
        for token in tokens {
            assert!(
                !message.contains(token),
                "case {case:?}: nothing of the request is interpolated, but {token:?} \
                 appears in {message:?}"
            );
        }
        // A refused declaration is not stored.
        let (status, listed) = emulator.cp("ListPipelines", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListPipelines must answer, got {status} with {listed}"
        );
        assert!(
            !listed["pipelines"].as_array().is_some_and(|summaries| {
                summaries
                    .iter()
                    .any(|summary| summary["name"] == json!(name))
            }),
            "case {case:?}: the refused pipeline must be absent from ListPipelines, got \
             {listed}"
        );
    }

    // The control: the SAME two stages with the source stage first are accepted, so what
    // the refusal turns on is the first stage's composition and not the stages
    // themselves.
    let accepted = declaration(
        "first-stage-control",
        json!([
            source_stage("SourceArtifact"),
            {"name": "Gate", "actions": [
                categorised_action("Approve", "Approval", "Manual", 1),
            ]},
        ]),
    );
    let (status, answer) = emulator.cp("CreatePipeline", &accepted).await;
    assert_eq!(
        status, 200,
        "a declaration whose first stage is the source stage is accepted, got {status} \
         with {answer}"
    );

    // `UpdatePipeline` takes the same declaration, so it words the same law the same way.
    let mut update = accepted.clone();
    update["pipeline"]["stages"] = json!([
        {"name": "Gate", "actions": [
            categorised_action("Approve", "Approval", "Manual", 1),
        ]},
        source_stage("SourceArtifact"),
    ]);
    let (status, code, message) = emulator.refusal("UpdatePipeline", &update).await;
    assert_eq!(
        (status, code.as_str(), message.as_str()),
        (400, "InvalidStructureException", FIRST_STAGE_MESSAGE),
        "UpdatePipeline refuses the same law with the same fixed sentence, got {status} \
         {code} {message:?}"
    );
}
