// SPDX-License-Identifier: CC0-1.0

//! Cover for what `DeleteWebhook` answers when the project it names holds no webhook,
//! driven over the wire the router serves: `POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1.
//!
//! The rule under test: the only resource `DeleteWebhook` addresses by name is the
//! PROJECT, so that is the one absence it is refused for — a project this account holds
//! that carries no webhook has nothing to disconnect, so the call is served with the
//! empty output, leaves the project (its `lastModified` included) exactly as it was, and
//! can be repeated.
//!
//! The rows span what the rule generalises over: every `SourceType` the model declares
//! (`crate::ops::types::SOURCE_TYPE_VALUES`), including the ones whose projects cannot
//! carry a webhook at all; every way a project comes to hold none (never connected,
//! connected then disconnected, and disconnected with the provider's access token given
//! back afterwards); and the number of times the call is repeated. The contrast row —
//! the absent PROJECT, which stays refused — keeps the two absences distinguishable.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::SOURCE_TYPE_VALUES;
use service_com_amazonaws_codebuild_codebuild_20161006::state::{
    self, ComAmazonawsCodebuildCodebuild20161006State as CodeBuildState,
};
use smithy_server_runtime::MultiServiceRouter;

use super::project_build_lifecycle::{
    call, create_project_body, emulator, emulator_on, error_type, source_of,
};

/// How a project came to hold no webhook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Absence {
    /// A project that has never been connected to its repository's events.
    NeverConnected,
    /// A project that was connected and then disconnected by an earlier delete.
    Disconnected,
    /// As [`Absence::Disconnected`], with the provider's access token given back
    /// afterwards: nothing of CodeBuild's is registered with the provider any more, so
    /// the token the provider half of a delete needs is not needed either.
    DisconnectedWithoutToken,
}

/// Whether a project whose primary source is `source_type` can carry a webhook at all:
/// the providers that send repository events (`state::WEBHOOK_SOURCE_TYPES`) plus the
/// source type the runner webhook belongs to (`state::RUNNER_WEBHOOK_SOURCE_TYPE`).
fn takes_a_webhook(source_type: &str) -> bool {
    state::WEBHOOK_SOURCE_TYPES.contains(&source_type)
        || source_type == state::RUNNER_WEBHOOK_SOURCE_TYPE
}

/// Create `name` with a primary source of `source_type`, asserting it succeeded.
async fn create_project(router: &MultiServiceRouter, name: &str, source_type: &str) {
    let (status, body) = call(
        router,
        "CreateProject",
        create_project_body(name, json!({"source": source_of(source_type)})),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?} ({source_type}): expected 200, got {status} with {body}"
    );
}

/// Import an access token for `server_type`, if that source type has a provider to
/// authenticate to, and hand back the ARN it is held under.
async fn import_credential(router: &MultiServiceRouter, server_type: &str) -> Option<String> {
    if !state::WEBHOOK_SOURCE_TYPES.contains(&server_type) {
        return None;
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
        "ImportSourceCredentials {server_type}: expected 200, got {status} with {body}"
    );
    body["arn"].as_str().map(str::to_string)
}

/// The project `BatchGetProjects` reports, asserting it is reported at all.
async fn read_project(router: &MultiServiceRouter, case: &str, project: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [project]})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: BatchGetProjects must be served, got {status} with {body}"
    );
    assert_eq!(
        body["projectsNotFound"],
        json!([]),
        "case {case:?}: the project must still be held, got {body}"
    );
    body["projects"][0].clone()
}

/// Connect `project` to its repository's events, asserting it succeeded.
async fn create_webhook(router: &MultiServiceRouter, case: &str, project: &str, source_type: &str) {
    let mut request = json!({"projectName": project});
    // The only webhook a project with no input source code takes is a runner one
    // (`RUNNER_BUILDKITE_BUILD` is "only available for NO_SOURCE source type projects").
    if source_type == state::RUNNER_WEBHOOK_SOURCE_TYPE {
        if let Value::Object(members) = &mut request {
            members.insert(
                "buildType".to_string(),
                json!(state::RUNNER_WEBHOOK_BUILD_TYPE),
            );
        }
    }
    let (status, body) = call(router, "CreateWebhook", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateWebhook must be served, got {status} with {body}"
    );
}

/// Assert `DeleteWebhook` answered the empty output: HTTP 200 with a body carrying none
/// of the members a webhook has, since `DeleteWebhookOutput` models no members at all.
fn assert_empty_success(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
        "case {case:?}: expected the empty DeleteWebhookOutput, got {body}"
    );
}

/// Every source type, every way a project comes to hold no webhook: the delete is served
/// with the empty output, repeats are served the same way, and the project is left
/// exactly as it was.
#[tokio::test]
async fn deleting_a_webhook_a_project_does_not_have_is_an_idempotent_no_op() {
    // (the project's source type, how it came to hold no webhook)
    let mut rows: Vec<(&str, Absence)> = Vec::new();
    for source_type in SOURCE_TYPE_VALUES {
        rows.push((source_type, Absence::NeverConnected));
        if takes_a_webhook(source_type) {
            rows.push((source_type, Absence::Disconnected));
            rows.push((source_type, Absence::DisconnectedWithoutToken));
        }
    }

    for (source_type, absence) in rows {
        let case = format!("a {source_type} project, {absence:?}");
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(Arc::clone(&engine));
        let project = format!(
            "dwn-{}-{}",
            source_type.to_ascii_lowercase(),
            match absence {
                Absence::NeverConnected => "never",
                Absence::Disconnected => "gone",
                Absence::DisconnectedWithoutToken => "gone-tokenless",
            }
        );
        create_project(&router, &project, source_type).await;
        let credential = import_credential(&router, source_type).await;

        if absence != Absence::NeverConnected {
            create_webhook(&router, &case, &project, source_type).await;
            let (status, body) =
                call(&router, "DeleteWebhook", json!({"projectName": project})).await;
            assert_empty_success(&format!("{case}: the delete that disconnected it"), status, &body);
        }
        if absence == Absence::DisconnectedWithoutToken {
            if let Some(arn) = credential {
                let (status, body) =
                    call(&router, "DeleteSourceCredentials", json!({"arn": arn})).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteSourceCredentials must be served, got {status} \
                     with {body}"
                );
            }
        }

        // What the project looks like with no webhook on it, before the call under test.
        let before = read_project(&router, &case, &project).await;
        assert!(
            before["webhook"].is_null(),
            "case {case:?}: the row must start with no webhook on the project, got {before}"
        );
        let stamp_before = engine
            .state::<CodeBuildState>()
            .lock()
            .projects
            .get(&project)
            .map(|stored| stored.last_modified_millis);

        // Repeating the call must answer the same way every time: nothing is consumed by
        // the first one, so there is nothing for a later one to run out of.
        for attempt in 1..=3 {
            let case = format!("{case}, attempt {attempt}");
            let (status, body) =
                call(&router, "DeleteWebhook", json!({"projectName": project})).await;
            assert_empty_success(&case, status, &body);

            let after = read_project(&router, &case, &project).await;
            assert_eq!(
                after, before,
                "case {case:?}: a no-op must leave the project untouched, got {after} where \
                 the project read {before}"
            );
            let stamp_after = engine
                .state::<CodeBuildState>()
                .lock()
                .projects
                .get(&project)
                .map(|stored| stored.last_modified_millis);
            assert_eq!(
                stamp_after, stamp_before,
                "case {case:?}: nothing changed, so the stored lastModified must stay at \
                 {stamp_before:?}, got {stamp_after:?}"
            );
        }

        // The project can still be connected afterwards, where its source type takes a
        // webhook at all: the no-op left nothing behind that a creation would trip on.
        if takes_a_webhook(source_type) {
            import_credential(&router, source_type).await;
            create_webhook(&router, &case, &project, source_type).await;
            let connected = read_project(&router, &case, &project).await;
            assert!(
                connected["webhook"]["payloadUrl"].is_string(),
                "case {case:?}: the project must take a webhook after the no-op, got \
                 {connected}"
            );
        }
    }
}

/// The contrast: the one absence the delete IS refused for is the project it addresses by
/// name, stated in CodeBuild's fixed absent-project words — so a client can still tell
/// "the project is gone" from "the project is there and simply has no webhook".
#[tokio::test]
async fn the_two_absences_stay_distinguishable() {
    let router = emulator();

    // Both forms of an absent project: one never created, one created and then deleted.
    create_project(&router, "dwd-removed", "NO_SOURCE").await;
    let (status, body) = call(
        &router,
        "DeleteProject",
        json!({"name": "dwd-removed"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteProject must be served, got {status} with {body}"
    );

    for project in ["dwd-never-created", "dwd-removed"] {
        let case = format!("DeleteWebhook on the absent project {project:?}");
        let (status, body) = call(&router, "DeleteWebhook", json!({"projectName": project})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(state::PROJECT_NOT_FOUND_MESSAGE),
            "case {case:?}: expected CodeBuild's absent-project sentence, got {body}"
        );
    }

    // And the project that IS held is served, with the sentence above nowhere in it.
    create_project(&router, "dwd-held", "NO_SOURCE").await;
    let (status, body) = call(
        &router,
        "DeleteWebhook",
        json!({"projectName": "dwd-held"}),
    )
    .await;
    assert_empty_success("DeleteWebhook on a held project with no webhook", status, &body);
}
