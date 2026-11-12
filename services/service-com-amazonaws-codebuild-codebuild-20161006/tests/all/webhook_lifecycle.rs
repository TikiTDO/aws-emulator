// SPDX-License-Identifier: CC0-1.0

//! Cover for `CreateWebhook`, `UpdateWebhook` and `DeleteWebhook`, driven over the wire
//! the router serves: `POST /` with `X-Amz-Target: CodeBuild_20161006.<Operation>` on
//! awsJson1_1.
//!
//! The rule under test is that a webhook is a record kept against a project whose
//! source provider sends repository events: it is created once per project, computed
//! from the project the request names, and reported by a later read exactly as it was
//! persisted — bar the `secret`, which the call that MINTS it reports and a read
//! withholds (see `tests/all/webhook_secret_is_minted_once.rs`). An update rewrites that
//! record's configuration from the request it
//! carries — the request IS the configuration afterwards, so a member it omits is
//! cleared — while the members identifying the created webhook, and its secret unless
//! `rotateSecret` asks for a new one, are left alone. Every table below spans a
//! catalogue the model declares, not one recorded request.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for — deliberately not the emulator's
/// `us-east-1` default, so a URL assertion proves the region came from the request.
const REGION: &str = "us-west-2";

/// The access key id every request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDCODEBUILDWEBHOOK";

/// Every `SourceType` the model declares, from
/// `smithy_query(query="shape", shape="SourceType")`, paired with whether a
/// repository-event webhook can be created for a project with that source.
const SOURCE_TYPES: &[(&str, bool)] = &[
    ("BITBUCKET", true),
    ("CODECOMMIT", false),
    ("CODEPIPELINE", false),
    ("GITHUB", true),
    ("GITHUB_ENTERPRISE", true),
    ("GITLAB", true),
    ("GITLAB_SELF_MANAGED", true),
    ("NO_SOURCE", false),
    ("S3", false),
];

/// Every `WebhookBuildType` the model declares, from
/// `smithy_query(query="shape", shape="WebhookBuildType")`, with the source type each
/// one is available for: `RUNNER_BUILDKITE_BUILD` is documented as "only available for
/// NO_SOURCE source type projects".
const BUILD_TYPES: &[(&str, &str)] = &[
    ("BUILD", "GITHUB"),
    ("BUILD_BATCH", "GITHUB"),
    ("RUNNER_BUILDKITE_BUILD", "NO_SOURCE"),
];

/// One router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    emulator_on(Arc::new(engine::InMemoryEngine::new()))
}

/// As [`emulator`], over an engine the caller already holds — so a test can inspect the
/// service's state through the very store the handlers write.
fn emulator_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and decoded body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/codebuild/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0f0f"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let decoded = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// Create a project named `name` whose source is `source_type`, asserting it succeeded.
///
/// A webhook call CodeBuild carries out against the repository provider needs an access
/// token for it, which is its own rule (see `tests/all/webhook_access_token.rs`), so a
/// project whose source has such a provider is given one here — leaving every case below
/// to be about the webhook rule it is written for.
async fn create_project(router: &MultiServiceRouter, name: &str, source_type: &str) {
    let (status, body) = call(
        router,
        "CreateProject",
        json!({
            "name": name,
            "source": source_of(source_type),
            "artifacts": {"type": "NO_ARTIFACTS"},
            "environment": {
                "type": "LINUX_CONTAINER",
                "image": "aws/codebuild/standard:7.0",
                "computeType": "BUILD_GENERAL1_SMALL",
            },
            "serviceRole": "arn:aws:iam::000000000000:role/service-role/codebuild",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?} ({source_type}): expected 200, got {status} with {body}"
    );
    import_source_credentials(router, source_type).await;
}

// A project source of `source_type` that satisfies the location rule, taken from the
// shared harness so this suite does not restate it (see
// `project_build_lifecycle::source_of`).
use super::project_build_lifecycle::source_of;

/// Hold an access token for `server_type` — one of the model's `ServerType` values, so
/// nothing is imported for a source type that has no repository provider.
async fn import_source_credentials(router: &MultiServiceRouter, server_type: &str) {
    if !state::WEBHOOK_SOURCE_TYPES.contains(&server_type) {
        return;
    }
    let (status, body) = call(
        router,
        "ImportSourceCredentials",
        json!({
            "serverType": server_type,
            "authType": "PERSONAL_ACCESS_TOKEN",
            "token": "an-access-token",
            "shouldOverwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ImportSourceCredentials ({server_type}): expected 200, got {status} with {body}"
    );
}

/// The `__type` an error body carries, for asserting the error code a client branches on.
fn error_type(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The webhook a creating call reported, as a READ of the project reports it: the same
/// record with `secret` withheld.
///
/// The secret is handed to the call that MINTED it and to no other, so a round trip
/// compares a read against this rather than against the creating call's own answer.
fn webhook_as_read(created: &Value) -> Value {
    let mut reported = created.clone();
    if let Some(members) = reported.as_object_mut() {
        members.remove("secret");
    }
    reported
}

/// The project a later read reports, through the implemented reader `BatchGetProjects`.
async fn read_project(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [name]})).await;
    assert_eq!(
        status, 200,
        "BatchGetProjects {name:?}: expected 200, got {status} with {body}"
    );
    body["projects"][0].clone()
}

/// A created webhook's shape, checked against what the model and the docs say the
/// members mean, for a project whose source is `source_type`.
fn assert_webhook_shape(case: &str, webhook: &Value, source_type: &str) {
    let payload_url = webhook["payloadUrl"].as_str().unwrap_or_default();
    assert!(
        payload_url.starts_with(&format!("https://codebuild.{REGION}.amazonaws.com/webhooks?t="))
            && payload_url.ends_with("&v=1"),
        "{case}: expected a regional CodeBuild payload URL, got {payload_url:?}"
    );
    assert!(
        webhook["url"].is_null(),
        "{case}: no hook is registered with the repository provider, so url must be \
         absent, got {:?}",
        webhook["url"]
    );
    assert_eq!(
        webhook["status"].as_str(),
        Some("ACTIVE"),
        "{case}: expected the persisted webhook to report ACTIVE, got {:?}",
        webhook["status"]
    );
    // AWS documents it on `Webhook$secret`: "A Bitbucket webhook does not support
    // secret." Every other provider's webhook carries one, and `lastModifiedSecret`
    // reports when that secret last changed, so the two members travel together.
    let expects_secret = source_type != "BITBUCKET";
    assert_eq!(
        webhook["secret"].is_string(),
        expects_secret,
        "{case}: expected secret present={expects_secret}, got {:?}",
        webhook["secret"]
    );
    assert_eq!(
        !webhook["lastModifiedSecret"].is_null(),
        expects_secret,
        "{case}: expected lastModifiedSecret present={expects_secret}, got {:?}",
        webhook["lastModifiedSecret"]
    );
}

/// The whole catalogue of source types: a webhook can be created exactly for the
/// providers that send repository events, and the created record is what a later read
/// reports.
#[tokio::test]
async fn create_webhook_spans_every_source_type() {
    let router = emulator();
    // The secret each creating call minted, kept per project: a later read withholds it,
    // so the only place to compare two projects' secrets is the calls that minted them.
    let mut created_secrets: std::collections::BTreeMap<String, Value> =
        std::collections::BTreeMap::new();

    for (source_type, supported) in SOURCE_TYPES {
        let case = format!("source type {source_type}");
        let project = format!("webhook-{}", source_type.to_ascii_lowercase());
        create_project(&router, &project, source_type).await;

        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "branchFilter": "^refs/heads/main$"}),
        )
        .await;

        if !supported {
            assert_eq!(
                status, 400,
                "{case}: expected 400, got {status} with {body}"
            );
            assert!(
                error_type(&body).contains("InvalidInputException"),
                "{case}: expected InvalidInputException, got {body}"
            );
            assert!(
                read_project(&router, &project).await["webhook"].is_null(),
                "{case}: a refused CreateWebhook must persist nothing"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {body}"
        );
        let webhook = body["webhook"].clone();
        assert_webhook_shape(&case, &webhook, source_type);
        assert_eq!(
            webhook["branchFilter"].as_str(),
            Some("^refs/heads/main$"),
            "{case}: expected the request's branchFilter, got {webhook}"
        );

        // Round trip: the project reports exactly the webhook that was created — bar the
        // secret, which the minting call reports and a read withholds — and reads do not
        // recompute it.
        let stored = read_project(&router, &project).await["webhook"].clone();
        assert_eq!(
            stored,
            webhook_as_read(&webhook),
            "{case}: BatchGetProjects must report the created webhook, got {stored}"
        );
        let again = read_project(&router, &project).await["webhook"].clone();
        assert_eq!(
            again, stored,
            "{case}: a second read must report the same webhook, got {again}"
        );
        created_secrets.insert(project.clone(), webhook["secret"].clone());
    }

    // Each project's payload URL is its own, and so is the secret each creating call
    // minted.
    let first = read_project(&router, "webhook-github").await["webhook"].clone();
    let second = read_project(&router, "webhook-gitlab").await["webhook"].clone();
    assert_ne!(
        first["payloadUrl"], second["payloadUrl"],
        "two projects must not share a payload URL: {first} vs {second}"
    );
    assert_ne!(
        created_secrets["webhook-github"], created_secrets["webhook-gitlab"],
        "two projects must not share a webhook secret"
    );
}

/// A project takes at most one webhook: the second request is refused and changes
/// nothing.
#[tokio::test]
async fn a_second_webhook_for_one_project_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "webhook-twice", "GITHUB").await;

    let (status, body) = call(
        &router,
        "CreateWebhook",
        json!({"projectName": "webhook-twice"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "first CreateWebhook: expected 200, got {status} with {body}"
    );
    let created = body["webhook"].clone();

    // A repeat differing in its parameters is still a second webhook for the same
    // project, which is what makes it a conflict.
    for (case, request) in [
        ("an identical repeat", json!({"projectName": "webhook-twice"})),
        (
            "a repeat with other parameters",
            json!({
                "projectName": "webhook-twice",
                "branchFilter": "^refs/heads/release$",
                "buildType": "BUILD_BATCH",
            }),
        ),
    ] {
        let (status, body) = call(&router, "CreateWebhook", request).await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("ResourceAlreadyExistsException"),
            "{case}: expected ResourceAlreadyExistsException, got {body}"
        );
    }

    let stored = read_project(&router, "webhook-twice").await["webhook"].clone();
    assert_eq!(
        stored,
        webhook_as_read(&created),
        "a refused repeat must leave the first webhook untouched, got {stored}"
    );
    // And the store holds the webhook on the project itself, not a second copy.
    let state = engine.state::<CodeBuildState>();
    let guard = state.lock();
    let project = guard
        .projects
        .get("webhook-twice")
        .expect("the project is persisted");
    assert_eq!(
        project.project.webhook.as_ref().and_then(|w| w.secret.clone()),
        created["secret"].as_str().map(str::to_string),
        "the persisted webhook must carry the secret the caller was given"
    );
}

/// No project, no webhook.
#[tokio::test]
async fn create_webhook_for_an_unknown_project_is_refused() {
    let router = emulator();
    let (status, body) = call(
        &router,
        "CreateWebhook",
        json!({"projectName": "no-such-project"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "CreateWebhook on an unknown project: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("ResourceNotFoundException"),
        "CreateWebhook on an unknown project: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["message"],
        json!(state::PROJECT_NOT_FOUND_MESSAGE),
        "the refusal must state that the project is the absent resource, got {body}"
    );
}

/// Every `WebhookBuildType`, each against the source type it is available for and one
/// it is not: a runner webhook needs a `NO_SOURCE` project, the repository-event build
/// types need a repository.
#[tokio::test]
async fn build_type_must_match_the_project_source() {
    let router = emulator();

    for (build_type, source_type) in BUILD_TYPES {
        let case = format!("buildType {build_type} on {source_type}");
        let project = format!("bt-ok-{}", build_type.to_ascii_lowercase());
        create_project(&router, &project, source_type).await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "buildType": build_type}),
        )
        .await;
        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["webhook"]["buildType"].as_str(),
            Some(*build_type),
            "{case}: expected the request's buildType, got {body}"
        );
        assert_webhook_shape(&case, &body["webhook"], source_type);

        // The same build type against a source it is not available for.
        let other_source = if *source_type == "NO_SOURCE" {
            "GITHUB"
        } else {
            "NO_SOURCE"
        };
        let case = format!("buildType {build_type} on {other_source}");
        let project = format!("bt-bad-{}", build_type.to_ascii_lowercase());
        create_project(&router, &project, other_source).await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "buildType": build_type}),
        )
        .await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "{case}: expected InvalidInputException, got {body}"
        );
    }
}

/// `filterGroups`: a group has to carry an `EVENT` filter to be satisfiable, and the
/// groups a caller gives are persisted as given.
#[tokio::test]
async fn filter_groups_need_an_event_filter() {
    let router = emulator();

    // Accepted: one group, several groups, and a group combining EVENT with another
    // filter type — the filter types come from the model's `WebhookFilterType`.
    let accepted: &[(&str, Value)] = &[
        (
            "one EVENT group",
            json!([[{"type": "EVENT", "pattern": "PUSH"}]]),
        ),
        (
            "two EVENT groups",
            json!([
                [{"type": "EVENT", "pattern": "PUSH"}],
                [{"type": "EVENT", "pattern": "PULL_REQUEST_CREATED,PULL_REQUEST_UPDATED"}],
            ]),
        ),
        (
            "EVENT with a HEAD_REF filter",
            json!([[
                {"type": "EVENT", "pattern": "PUSH"},
                {"type": "HEAD_REF", "pattern": "^refs/heads/main$", "excludeMatchedPattern": true},
            ]]),
        ),
    ];
    for (index, (case, filter_groups)) in accepted.iter().enumerate() {
        let project = format!("fg-ok-{index}");
        create_project(&router, &project, "GITHUB").await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "filterGroups": filter_groups}),
        )
        .await;
        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            &body["webhook"]["filterGroups"], filter_groups,
            "{case}: expected the request's filterGroups, got {body}"
        );
        assert_eq!(
            read_project(&router, &project).await["webhook"]["filterGroups"],
            *filter_groups,
            "{case}: a later read must report the stored filterGroups"
        );
    }

    // Refused: any group without an EVENT filter, wherever it sits in the array.
    let refused: &[(&str, Value)] = &[
        (
            "a lone HEAD_REF group",
            json!([[{"type": "HEAD_REF", "pattern": "^refs/heads/main$"}]]),
        ),
        (
            "an empty group",
            json!([[]]),
        ),
        (
            "a second group without EVENT",
            json!([
                [{"type": "EVENT", "pattern": "PUSH"}],
                [{"type": "FILE_PATH", "pattern": "^src/"}],
            ]),
        ),
    ];
    for (index, (case, filter_groups)) in refused.iter().enumerate() {
        let project = format!("fg-bad-{index}");
        create_project(&router, &project, "GITHUB").await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "filterGroups": filter_groups}),
        )
        .await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "{case}: expected InvalidInputException, got {body}"
        );
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("filterGroups"),
            "{case}: the refusal must name the member it is about, got {body}"
        );
        assert!(
            read_project(&router, &project).await["webhook"].is_null(),
            "{case}: a refused CreateWebhook must persist nothing"
        );
    }
}

/// `manualCreation` is "only available for GitHub webhooks": accepted there, refused
/// for every other provider that takes a webhook at all.
#[tokio::test]
async fn manual_creation_is_github_only() {
    let router = emulator();

    for (source_type, supported) in SOURCE_TYPES.iter().filter(|(_, supported)| *supported) {
        let case = format!("manualCreation on {source_type}");
        let project = format!("mc-{}", source_type.to_ascii_lowercase());
        create_project(&router, &project, source_type).await;
        let (status, body) = call(
            &router,
            "CreateWebhook",
            json!({"projectName": project, "manualCreation": true}),
        )
        .await;
        let _ = supported;
        if *source_type == "GITHUB" {
            assert_eq!(
                status, 200,
                "{case}: expected 200, got {status} with {body}"
            );
            assert_eq!(
                body["webhook"]["manualCreation"].as_bool(),
                Some(true),
                "{case}: expected the request's manualCreation, got {body}"
            );
            // A manually created webhook is exactly the pair of values the caller
            // registers by hand with the provider.
            assert!(
                body["webhook"]["payloadUrl"].is_string() && body["webhook"]["secret"].is_string(),
                "{case}: expected payloadUrl and secret, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "{case}: expected 400, got {status} with {body}"
            );
            assert!(
                error_type(&body).contains("InvalidInputException"),
                "{case}: expected InvalidInputException, got {body}"
            );
        }
    }
}

/// The webhook belongs to the project, so connecting one is a change to the project's
/// settings — which is what `Project$lastModified` reports.
#[tokio::test]
async fn creating_a_webhook_touches_the_projects_last_modified() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project(&router, "webhook-stamped", "GITHUB").await;

    let created_at = engine
        .state::<CodeBuildState>()
        .lock()
        .projects
        .get("webhook-stamped")
        .map(|stored| (stored.created_millis, stored.last_modified_millis))
        .expect("the project is persisted");
    assert_eq!(
        created_at.0, created_at.1,
        "a freshly created project was last modified when it was created"
    );

    // Rewind the stored instants, so the assertion below is about the write and not
    // about how fast the test runs.
    {
        let state = engine.state::<CodeBuildState>();
        let mut guard = state.lock();
        let stored = guard
            .projects
            .get_mut("webhook-stamped")
            .expect("the project is persisted");
        stored.created_millis -= 60_000;
        stored.last_modified_millis -= 60_000;
        stored.project.created = Some(state::datetime(stored.created_millis));
        stored.project.last_modified = Some(state::datetime(stored.last_modified_millis));
    }

    let (status, body) = call(
        &router,
        "CreateWebhook",
        json!({"projectName": "webhook-stamped"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWebhook: expected 200, got {status} with {body}"
    );

    let guard = engine.state::<CodeBuildState>();
    let guard = guard.lock();
    let stored = guard
        .projects
        .get("webhook-stamped")
        .expect("the project is persisted");
    assert!(
        stored.last_modified_millis > stored.created_millis,
        "expected lastModified to move past created, got {} vs {}",
        stored.last_modified_millis,
        stored.created_millis
    );
    assert_eq!(
        stored.project.last_modified.map(|at| at.to_millis().unwrap_or_default()),
        Some(stored.last_modified_millis),
        "the reported lastModified must be the instant the store holds"
    );
}

// --- UpdateWebhook ------------------------------------------------------------------

/// The configuration members `UpdateWebhook` models, from
/// `smithy_query(query="operation", operation="UpdateWebhook")`: everything in its input
/// beside the `projectName` that identifies the webhook and the `rotateSecret` flag.
const CONFIGURATION_MEMBERS: &[&str] = &[
    "branchFilter",
    "filterGroups",
    "buildType",
    "pullRequestBuildPolicy",
];

/// The members that identify the webhook CodeBuild created, which no update may touch:
/// `UpdateWebhook`'s input models none of them.
const IDENTITY_MEMBERS: &[&str] = &["url", "payloadUrl", "manualCreation", "status"];

/// A webhook configuration that sets every member [`CONFIGURATION_MEMBERS`] names, so a
/// later update can be seen to replace — and to clear — each one. The enum values are
/// the model's: `WebhookFilterType`, `WebhookBuildType`,
/// `PullRequestBuildCommentApproval` and `PullRequestBuildApproverRole`.
fn full_configuration() -> Value {
    json!({
        "branchFilter": "^refs/heads/main$",
        "filterGroups": [[
            {"type": "EVENT", "pattern": "PUSH,PULL_REQUEST_CREATED"},
            {"type": "HEAD_REF", "pattern": "^refs/heads/main$"},
        ]],
        "buildType": "BUILD",
        "pullRequestBuildPolicy": {
            "requiresCommentApproval": "ALL_PULL_REQUESTS",
            "approverRoles": ["GITHUB_WRITE"],
        },
    })
}

/// Create a project with `source_type` and connect `configuration` to it as its
/// webhook, returning the webhook the creating call reported.
async fn create_project_with_webhook(
    router: &MultiServiceRouter,
    project: &str,
    source_type: &str,
    mut configuration: Value,
) -> Value {
    create_project(router, project, source_type).await;
    configuration["projectName"] = json!(project);
    let (status, body) = call(router, "CreateWebhook", configuration).await;
    assert_eq!(
        status, 200,
        "CreateWebhook {project:?}: expected 200, got {status} with {body}"
    );
    body["webhook"].clone()
}

/// Call `UpdateWebhook` for `project` with `request` as the rest of the input.
async fn update_webhook(
    router: &MultiServiceRouter,
    project: &str,
    mut request: Value,
) -> (u16, Value) {
    request["projectName"] = json!(project);
    call(router, "UpdateWebhook", request).await
}

/// Assert that `webhook` reports the same configuration as `expected`, member by member.
fn assert_same_configuration(case: &str, webhook: &Value, expected: &Value) {
    for member in CONFIGURATION_MEMBERS {
        assert_eq!(
            webhook[*member], expected[*member],
            "{case}: expected {member} {}, got {}",
            expected[*member], webhook[*member]
        );
    }
}

/// The secret the store holds for `project`'s webhook, and the instant it reports as
/// that secret's last modification, in milliseconds.
fn stored_secret(
    engine: &Arc<engine::InMemoryEngine>,
    project: &str,
) -> (Option<String>, Option<i64>) {
    let state = engine.state::<CodeBuildState>();
    let guard = state.lock();
    let webhook = guard
        .projects
        .get(project)
        .and_then(|stored| stored.project.webhook.clone());
    let secret = webhook.as_ref().and_then(|webhook| webhook.secret.clone());
    let stamp = webhook
        .as_ref()
        .and_then(|webhook| webhook.last_modified_secret)
        .and_then(|at| at.to_millis().ok());
    (secret, stamp)
}

/// Move `project`'s webhook secret stamp a minute into the past, so an assertion about
/// a rotation is about the write and not about how fast the test runs. Returns the
/// stamp the store holds afterwards.
fn rewind_secret_stamp(engine: &Arc<engine::InMemoryEngine>, project: &str) -> Option<i64> {
    let state = engine.state::<CodeBuildState>();
    let mut guard = state.lock();
    let webhook = guard
        .projects
        .get_mut(project)
        .and_then(|stored| stored.project.webhook.as_mut())?;
    let rewound = webhook
        .last_modified_secret
        .and_then(|at| at.to_millis().ok())
        .map(|at| at - 60_000)?;
    webhook.last_modified_secret = Some(state::datetime(rewound));
    Some(rewound)
}

/// Every configuration member the input models, set on its own and all at once: the
/// request is the webhook's configuration afterwards, so each member it carries is
/// stored and each one it omits is cleared, while the members identifying the created
/// webhook are left exactly as they were.
#[tokio::test]
async fn an_update_replaces_the_configuration_it_models() {
    // One row per member of [`CONFIGURATION_MEMBERS`], plus a request that carries no
    // configuration at all and one that carries every member.
    let cases: &[(&str, Value)] = &[
        (
            "branchFilter alone",
            json!({"branchFilter": "^refs/heads/release$"}),
        ),
        (
            "filterGroups alone",
            json!({"filterGroups": [
                [{"type": "EVENT", "pattern": "PULL_REQUEST_MERGED"}],
                [{"type": "EVENT", "pattern": "PUSH"}, {"type": "FILE_PATH", "pattern": "^src/"}],
            ]}),
        ),
        ("buildType alone", json!({"buildType": "BUILD_BATCH"})),
        (
            "pullRequestBuildPolicy alone",
            json!({"pullRequestBuildPolicy": {
                "requiresCommentApproval": "FORK_PULL_REQUESTS",
                "approverRoles": ["GITHUB_ADMIN", "GITHUB_MAINTAIN"],
            }}),
        ),
        ("no configuration at all", json!({})),
        ("every member at once", full_configuration()),
    ];

    for (index, (case, request)) in cases.iter().enumerate() {
        let router = emulator();
        let project = format!("uw-set-{index}");
        let created =
            create_project_with_webhook(&router, &project, "GITHUB", full_configuration()).await;
        assert_same_configuration(
            &format!("{case}: the created webhook"),
            &created,
            &full_configuration(),
        );

        let (status, body) = update_webhook(&router, &project, request.clone()).await;
        assert_eq!(
            status, 200,
            "{case}: expected 200, got {status} with {body}"
        );
        let updated = body["webhook"].clone();

        assert_same_configuration(case, &updated, request);
        for member in IDENTITY_MEMBERS {
            assert_eq!(
                updated[*member], created[*member],
                "{case}: {member} identifies the created webhook and must be left alone, \
                 got {} where the creating call reported {}",
                updated[*member], created[*member]
            );
        }

        // Round trip: the project reports exactly the configuration the update stored,
        // and a second read reports the same again.
        let stored = read_project(&router, &project).await["webhook"].clone();
        assert_same_configuration(&format!("{case}: read back"), &stored, request);
        let again = read_project(&router, &project).await["webhook"].clone();
        assert_eq!(
            again, stored,
            "{case}: a second read must report the same webhook, got {again}"
        );
    }
}

/// `rotateSecret` across every provider that takes a webhook: a new secret is minted
/// exactly when the caller asks for one and the provider supports one at all, it is
/// reported only by the call that minted it, and the stamp moves only then.
#[tokio::test]
async fn rotate_secret_mints_a_new_secret_only_when_asked() {
    for (source_type, _) in SOURCE_TYPES.iter().filter(|(_, supported)| *supported) {
        for rotate in [false, true] {
            let case = format!("rotateSecret {rotate} on {source_type}");
            let engine = Arc::new(engine::InMemoryEngine::new());
            let router = emulator_on(Arc::clone(&engine));
            let project = format!("uw-rot-{}-{rotate}", source_type.to_ascii_lowercase());
            let created =
                create_project_with_webhook(&router, &project, source_type, json!({})).await;

            // AWS documents on `Webhook$secret` that "a Bitbucket webhook does not
            // support secret", and on `rotateSecret` that it "is ignored" there.
            let carries_secret = *source_type != "BITBUCKET";
            let rotated = rotate && carries_secret;

            let (before_secret, _) = stored_secret(&engine, &project);
            let before_stamp = rewind_secret_stamp(&engine, &project);
            assert_eq!(
                before_secret.is_some(),
                carries_secret,
                "{case}: expected the created webhook's stored secret present={carries_secret}, \
                 got {before_secret:?}"
            );

            let (status, body) =
                update_webhook(&router, &project, json!({"rotateSecret": rotate})).await;
            assert_eq!(
                status, 200,
                "{case}: expected 200, got {status} with {body}"
            );
            let updated = body["webhook"].clone();
            let (after_secret, after_stamp) = stored_secret(&engine, &project);

            assert_eq!(
                updated["secret"].is_string(),
                rotated,
                "{case}: the secret is reported by the call that minted it and by no \
                 other, so expected present={rotated}, got {:?}",
                updated["secret"]
            );
            if rotated {
                assert_eq!(
                    after_secret.as_deref(),
                    updated["secret"].as_str(),
                    "{case}: the stored secret must be the one the caller was handed, got \
                     {after_secret:?}"
                );
                assert_ne!(
                    after_secret, before_secret,
                    "{case}: a rotation must replace the stored secret, got \
                     {after_secret:?}"
                );
                assert!(
                    after_stamp.unwrap_or_default() > before_stamp.unwrap_or_default(),
                    "{case}: expected lastModifiedSecret to move past {before_stamp:?}, got \
                     {after_stamp:?}"
                );
            } else {
                assert_eq!(
                    after_secret, before_secret,
                    "{case}: nothing asked for a new secret, so the stored one must stand, \
                     got {after_secret:?}"
                );
                assert_eq!(
                    after_stamp, before_stamp,
                    "{case}: lastModifiedSecret reports when the secret last changed, so it \
                     must not move here, got {after_stamp:?}"
                );
            }
            assert_eq!(
                updated["lastModifiedSecret"].is_null(),
                after_stamp.is_none(),
                "{case}: the reported lastModifiedSecret must be the one the store holds, \
                 got {:?} against {after_stamp:?}",
                updated["lastModifiedSecret"]
            );
            // Whatever the secret did, the webhook that was created is still this
            // project's webhook.
            for member in IDENTITY_MEMBERS {
                assert_eq!(
                    updated[*member], created[*member],
                    "{case}: {member} must be left as created, got {}",
                    updated[*member]
                );
            }
        }
    }
}

/// What an update cannot apply is refused, and a refused update changes nothing: the
/// project or the webhook is missing, a filter group can never be satisfied, or the
/// build type does not go with the project's source.
#[tokio::test]
async fn an_update_refuses_what_it_cannot_apply() {
    // No project, no webhook to update.
    let router = emulator();
    let (status, body) = update_webhook(&router, "no-such-project", json!({})).await;
    assert_eq!(
        status, 400,
        "UpdateWebhook on an unknown project: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("ResourceNotFoundException"),
        "UpdateWebhook on an unknown project: expected ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        body["message"],
        json!(state::PROJECT_NOT_FOUND_MESSAGE),
        "the refusal must state that the project is the absent resource, got {body}"
    );

    // A project that has no webhook has no configuration to rewrite.
    create_project(&router, "uw-no-webhook", "GITHUB").await;
    let (status, body) = update_webhook(
        &router,
        "uw-no-webhook",
        json!({"branchFilter": "^refs/heads/main$"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "UpdateWebhook on a project with no webhook: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("ResourceNotFoundException"),
        "UpdateWebhook on a project with no webhook: expected ResourceNotFoundException, \
         got {body}"
    );
    assert!(
        read_project(&router, "uw-no-webhook").await["webhook"].is_null(),
        "a refused UpdateWebhook must persist nothing"
    );

    // A configuration no webhook event can ever satisfy, and a build type the project's
    // source does not take. Each row states what its refusal must say: a filter-group
    // refusal names the input member it is about, while a build-type refusal is
    // CodeBuild's own fixed sentence for the build-type/source-type pairing, which names
    // only the project's stored source type.
    let refused: &[(&str, Value, &str)] = &[
        (
            "a lone HEAD_REF group",
            json!({"filterGroups": [[{"type": "HEAD_REF", "pattern": "^refs/heads/main$"}]]}),
            "filterGroups",
        ),
        ("an empty group", json!({"filterGroups": [[]]}), "filterGroups"),
        (
            "a second group without EVENT",
            json!({"filterGroups": [
                [{"type": "EVENT", "pattern": "PUSH"}],
                [{"type": "FILE_PATH", "pattern": "^src/"}],
            ]}),
            "filterGroups",
        ),
        (
            "a runner build type on a repository source",
            json!({"buildType": "RUNNER_BUILDKITE_BUILD"}),
            "Invalid build type provided for GITHUB type webhook",
        ),
    ];
    for (index, (case, request, stated)) in refused.iter().enumerate() {
        let router = emulator();
        let project = format!("uw-bad-{index}");
        let created =
            create_project_with_webhook(&router, &project, "GITHUB", full_configuration()).await;
        let (status, body) = update_webhook(&router, &project, request.clone()).await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "{case}: expected InvalidInputException, got {body}"
        );
        assert!(
            body["message"].as_str().unwrap_or_default().contains(stated),
            "{case}: the refusal must state {stated:?}, got {body}"
        );
        assert_same_configuration(
            &format!("{case}: after the refusal"),
            &read_project(&router, &project).await["webhook"],
            &created,
        );
    }

    // The converse of the runner rule: a `NO_SOURCE` project takes a runner webhook and
    // nothing else, so moving it to a repository-event build type is refused too.
    let router = emulator();
    create_project_with_webhook(
        &router,
        "uw-runner",
        "NO_SOURCE",
        json!({"buildType": "RUNNER_BUILDKITE_BUILD"}),
    )
    .await;
    for build_type in ["BUILD", "BUILD_BATCH"] {
        let case = format!("buildType {build_type} on a NO_SOURCE project");
        let (status, body) =
            update_webhook(&router, "uw-runner", json!({"buildType": build_type})).await;
        assert_eq!(
            status, 400,
            "{case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_type(&body).contains("InvalidInputException"),
            "{case}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            read_project(&router, "uw-runner").await["webhook"]["buildType"].as_str(),
            Some("RUNNER_BUILDKITE_BUILD"),
            "{case}: a refused update must leave the stored build type alone"
        );
    }
}

/// Rewriting the webhook changes the project's settings, which is what
/// `Project$lastModified` reports — and two updates never report the same instant.
#[tokio::test]
async fn an_update_touches_the_projects_last_modified() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project_with_webhook(&router, "uw-stamped", "GITHUB", full_configuration()).await;

    // Rewind the stored instants, so the assertion below is about the write and not
    // about how fast the test runs.
    {
        let state = engine.state::<CodeBuildState>();
        let mut guard = state.lock();
        let stored = guard
            .projects
            .get_mut("uw-stamped")
            .expect("the project is persisted");
        stored.created_millis -= 60_000;
        stored.last_modified_millis -= 60_000;
        stored.project.created = Some(state::datetime(stored.created_millis));
        stored.project.last_modified = Some(state::datetime(stored.last_modified_millis));
    }

    let mut stamps = Vec::new();
    for branch_filter in ["^refs/heads/main$", "^refs/heads/release$"] {
        let (status, body) =
            update_webhook(&router, "uw-stamped", json!({"branchFilter": branch_filter})).await;
        assert_eq!(
            status, 200,
            "UpdateWebhook {branch_filter}: expected 200, got {status} with {body}"
        );
        let state = engine.state::<CodeBuildState>();
        let guard = state.lock();
        let stored = guard
            .projects
            .get("uw-stamped")
            .expect("the project is persisted");
        assert!(
            stored.last_modified_millis > stored.created_millis,
            "UpdateWebhook {branch_filter}: expected lastModified to move past created, \
             got {} vs {}",
            stored.last_modified_millis,
            stored.created_millis
        );
        assert_eq!(
            stored
                .project
                .last_modified
                .map(|at| at.to_millis().unwrap_or_default()),
            Some(stored.last_modified_millis),
            "UpdateWebhook {branch_filter}: the reported lastModified must be the instant \
             the store holds"
        );
        stamps.push(stored.last_modified_millis);
    }
    assert!(
        stamps[1] > stamps[0],
        "two updates must not report the same lastModified, got {stamps:?}"
    );
}

// --- DeleteWebhook ------------------------------------------------------------------

/// Disconnecting a project removes the webhook record and nothing else: a later read
/// reports no webhook, the project itself is untouched, and the project can be connected
/// again. The table spans every source type that takes a webhook at all, each with its
/// own configuration.
#[tokio::test]
async fn a_delete_removes_the_webhook_and_leaves_the_project() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));

    for (source_type, _) in SOURCE_TYPES.iter().filter(|(_, supported)| *supported) {
        let case = format!("source type {source_type}");
        let project = format!("dw-{}", source_type.to_ascii_lowercase());
        let created =
            create_project_with_webhook(&router, &project, source_type, full_configuration()).await;
        assert!(
            !created["payloadUrl"].as_str().unwrap_or_default().is_empty(),
            "{case}: the created webhook must carry a payload URL, got {created}"
        );

        let (status, body) = call(&router, "DeleteWebhook", json!({"projectName": project})).await;
        assert_eq!(
            status, 200,
            "{case}: expected 200 from DeleteWebhook, got {status} with {body}"
        );

        let read = read_project(&router, &project).await;
        assert!(
            read["webhook"].is_null(),
            "{case}: a deleted webhook must not be reported, got {}",
            read["webhook"]
        );
        assert_eq!(
            read["name"].as_str(),
            Some(project.as_str()),
            "{case}: the project itself must survive the delete, got {read}"
        );
        assert!(
            engine
                .state::<CodeBuildState>()
                .lock()
                .projects
                .get(&project)
                .is_some_and(|stored| stored.project.webhook.is_none()),
            "{case}: the store must hold the project with no webhook"
        );

        // A project with no webhook can be connected again: the delete left nothing of
        // the first record behind to refuse the second creation.
        let mut configuration = full_configuration();
        configuration["projectName"] = json!(project);
        let (status, body) = call(&router, "CreateWebhook", configuration).await;
        assert_eq!(
            status, 200,
            "{case}: expected 200 re-creating the webhook, got {status} with {body}"
        );
        let again = body["webhook"].clone();
        assert_eq!(
            read_project(&router, &project).await["webhook"],
            webhook_as_read(&again),
            "{case}: the read must report the re-created webhook, got {again}"
        );
    }
}

/// A delete is refused for the one resource it addresses by name — the project — and is
/// served for a project it resolves, whether or not that project carries a webhook: the
/// detailed cover for that no-op is in `tests/all/webhook_delete_is_idempotent.rs`.
#[tokio::test]
async fn a_delete_refuses_only_an_absent_project() {
    let router = emulator();

    let (status, body) = call(
        &router,
        "DeleteWebhook",
        json!({"projectName": "no-such-project"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "DeleteWebhook on an unknown project: expected 400, got {status} with {body}"
    );
    assert!(
        error_type(&body).contains("ResourceNotFoundException"),
        "DeleteWebhook on an unknown project: expected ResourceNotFoundException, got {body}"
    );

    create_project(&router, "dw-no-webhook", "GITHUB").await;
    let (status, body) = call(
        &router,
        "DeleteWebhook",
        json!({"projectName": "dw-no-webhook"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteWebhook on a project with no webhook: expected 200, got {status} with {body}"
    );

    // And a second delete of a webhook that was already removed is served too.
    create_project_with_webhook(&router, "dw-twice", "GITHUB", full_configuration()).await;
    for attempt in 1..=2 {
        let (status, body) =
            call(&router, "DeleteWebhook", json!({"projectName": "dw-twice"})).await;
        assert_eq!(
            status, 200,
            "DeleteWebhook attempt {attempt}: expected 200, got {status} with {body}"
        );
    }
}

/// The webhook belongs to the project, so disconnecting one is a change to the project's
/// settings — which is what `Project$lastModified` reports.
#[tokio::test]
async fn a_delete_touches_the_projects_last_modified() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(Arc::clone(&engine));
    create_project_with_webhook(&router, "dw-stamped", "GITHUB", full_configuration()).await;

    // Rewind the stored instants, so the assertion below is about the write and not
    // about how fast the test runs.
    let created_at = {
        let state = engine.state::<CodeBuildState>();
        let mut guard = state.lock();
        let stored = guard
            .projects
            .get_mut("dw-stamped")
            .expect("the project is persisted");
        stored.created_millis -= 60_000;
        stored.last_modified_millis -= 60_000;
        stored.project.created = Some(state::datetime(stored.created_millis));
        stored.project.last_modified = Some(state::datetime(stored.last_modified_millis));
        stored.last_modified_millis
    };

    let (status, body) = call(
        &router,
        "DeleteWebhook",
        json!({"projectName": "dw-stamped"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteWebhook: expected 200, got {status} with {body}"
    );

    let state = engine.state::<CodeBuildState>();
    let guard = state.lock();
    let stored = guard
        .projects
        .get("dw-stamped")
        .expect("the project is persisted");
    assert!(
        stored.last_modified_millis > created_at,
        "DeleteWebhook: expected lastModified to move past {created_at}, got {}",
        stored.last_modified_millis
    );
    assert_eq!(
        stored
            .project
            .last_modified
            .map(|at| at.to_millis().unwrap_or_default()),
        Some(stored.last_modified_millis),
        "DeleteWebhook: the reported lastModified must be the instant the store holds"
    );
}
