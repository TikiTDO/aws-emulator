// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHICH resource a webhook operation reports as absent, driven over
//! the wire the router serves (`POST /` with
//! `X-Amz-Target: CodeBuild_20161006.<Operation>` on awsJson1_1) through the harness in
//! `project_build_lifecycle`.
//!
//! RULE under test: an operation that addresses a project's webhook by `projectName`
//! resolves THAT PROJECT before it evaluates any precondition about the webhook or about
//! the credentials CodeBuild would carry the provider-side call out with, so a name that
//! resolves to no project of this account is refused with CodeBuild's fixed
//! absent-project sentence — and the absent-webhook sentence is reserved for a project
//! that does resolve and carries no webhook.
//!
//! `CreateWebhook`, `UpdateWebhook` and `DeleteWebhook` all take `projectName`
//! (`ProjectName`, len 2..150, pattern `^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$`) as their
//! only project handle and all model `ResourceNotFoundException` at HTTP 400
//! (smithy_query(query="operation",operation="CreateWebhook,UpdateWebhook,DeleteWebhook")),
//! so one table spans the class.

use serde_json::{json, Value};
use service_com_amazonaws_codebuild_codebuild_20161006::state;

use crate::project_build_lifecycle::{
    call, create_project_body, emulator, error_type, source_of,
};

/// The source type every project below is created with: a repository provider whose hook
/// CodeBuild registers itself, so the access-token precondition is live on every row and
/// the ordering the rule states is what decides each answer.
const SOURCE_TYPE: &str = "GITHUB";

/// Hold a GitHub access token — the credential the provider-side half of a webhook call
/// needs, and the one whose absence has a refusal of its own.
async fn import_github_token(router: &smithy_server_runtime::MultiServiceRouter) {
    let (status, body) = call(
        router,
        "ImportSourceCredentials",
        json!({
            "serverType": SOURCE_TYPE,
            "authType": "PERSONAL_ACCESS_TOKEN",
            "token": "an-access-token",
            "shouldOverwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ImportSourceCredentials: expected 200, got {status} with {body}"
    );
}

/// Create a repository-sourced project, asserting it succeeded.
async fn create_project(router: &smithy_server_runtime::MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "CreateProject",
        create_project_body(name, json!({"source": source_of(SOURCE_TYPE)})),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateProject {name:?}: expected 200, got {status} with {body}"
    );
}

/// Connect `name` to its repository's events, asserting it succeeded.
///
/// `manualCreation` is what AWS documents as "CodeBuild doesn't create a webhook in
/// GitHub and instead returns payloadUrl and secret values", so this seeding call needs
/// no access token and a row can hold a webhook whether or not a token was imported.
async fn create_manual_webhook(router: &smithy_server_runtime::MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "CreateWebhook",
        json!({"projectName": name, "manualCreation": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateWebhook {name:?}: expected 200, got {status} with {body}"
    );
}

/// Delete `name`, asserting it succeeded.
async fn delete_project(router: &smithy_server_runtime::MultiServiceRouter, name: &str) {
    let (status, body) = call(router, "DeleteProject", json!({"name": name})).await;
    assert_eq!(
        status, 200,
        "DeleteProject {name:?}: expected 200, got {status} with {body}"
    );
}

/// Whether `name` resolves to no project at all, read back through the implemented
/// reader: `BatchGetProjects` reports a name it holds nothing for under
/// `projectsNotFound`.
async fn resolves_to_nothing(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> bool {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [name]})).await;
    assert_eq!(
        status, 200,
        "BatchGetProjects {name:?}: expected 200, got {status} with {body}"
    );
    body["projectsNotFound"]
        .as_array()
        .is_some_and(|missing| missing.iter().any(|entry| entry == name))
}

/// How a `projectName` comes to resolve to no project of this account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unresolvable {
    /// No project of that name was ever created.
    NeverCreated,
    /// A project of that name was created and deleted, having never had a webhook.
    DeletedWithNoWebhook,
    /// A project of that name was created, connected to its repository's events, and
    /// then deleted: the webhook record went with the project it belonged to.
    DeletedCarryingAWebhook,
    /// A project is held whose name differs from the requested one only in letter case —
    /// the `ProjectName` pattern admits both cases and a project is held under the name
    /// it was created with, so this names a different project, and that held sibling
    /// carries a webhook of its own.
    LetterCaseDiffersFromAHeldProject,
}

/// The name a row holds a project under, if any, and the name it then addresses.
fn names(form: Unresolvable, stem: &str) -> (Option<String>, String) {
    match form {
        Unresolvable::NeverCreated => (None, stem.to_string()),
        Unresolvable::DeletedWithNoWebhook | Unresolvable::DeletedCarryingAWebhook => {
            (Some(stem.to_string()), stem.to_string())
        }
        Unresolvable::LetterCaseDiffersFromAHeldProject => {
            (Some(stem.to_uppercase()), stem.to_string())
        }
    }
}

/// Bring one row's state about, and hand back the name the row addresses.
async fn seed(
    router: &smithy_server_runtime::MultiServiceRouter,
    form: Unresolvable,
    stem: &str,
) -> String {
    let (held, requested) = names(form, stem);
    if let Some(held) = held.as_deref() {
        create_project(router, held).await;
        if form != Unresolvable::DeletedWithNoWebhook {
            create_manual_webhook(router, held).await;
        }
        if form != Unresolvable::LetterCaseDiffersFromAHeldProject {
            delete_project(router, held).await;
        }
    }
    requested
}

/// The requests that address a project's webhook: the operation, and the members a row
/// sends beside `projectName`.
///
/// The variations are the preconditions that would otherwise decide the answer for a
/// project that DOES resolve: a build type the project's source does not take, filter
/// groups, a secret rotation, and the manual creation that skips the token check — so a
/// row proves the project is resolved before each of them.
fn requests() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("CreateWebhook", "minimal", json!({})),
        (
            "CreateWebhook",
            "manualCreation true",
            json!({"manualCreation": true}),
        ),
        (
            "CreateWebhook",
            "a runner build type",
            json!({"buildType": "RUNNER_BUILDKITE_BUILD"}),
        ),
        ("UpdateWebhook", "minimal", json!({})),
        (
            "UpdateWebhook",
            "rotateSecret true",
            json!({"rotateSecret": true}),
        ),
        (
            "UpdateWebhook",
            "a runner build type",
            json!({"buildType": "RUNNER_BUILDKITE_BUILD"}),
        ),
        (
            "UpdateWebhook",
            "an EVENT filter group",
            json!({"filterGroups": [[{"type": "EVENT", "pattern": "PUSH"}]]}),
        ),
        // `DeleteWebhookInput` models `projectName` and nothing else.
        ("DeleteWebhook", "minimal", json!({})),
    ]
}

/// One request body for `operation` against `project`, with `extra`'s members merged in.
fn request(project: &str, extra: &Value) -> Value {
    let mut body = json!({"projectName": project});
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extra.clone()) {
        target.extend(members);
    }
    body
}

/// THE RULE: on every webhook operation, a `projectName` that resolves to no project is
/// refused on the PROJECT — HTTP 400, `ResourceNotFoundException`, CodeBuild's fixed
/// absent-project sentence — whatever else the request carries and whether or not an
/// access token for the source's provider is held.
#[tokio::test]
async fn a_webhook_operation_on_an_unresolvable_project_reports_the_project() {
    let forms = [
        Unresolvable::NeverCreated,
        Unresolvable::DeletedWithNoWebhook,
        Unresolvable::DeletedCarryingAWebhook,
        Unresolvable::LetterCaseDiffersFromAHeldProject,
    ];

    for (nth, (operation, variation, extra)) in requests().iter().enumerate() {
        for form in forms {
            for token_held in [false, true] {
                let case = format!(
                    "{operation} with {variation} on a project {form:?} \
                     (access token held: {token_held})"
                );
                // A fresh emulator per row: no row's answer may depend on the one before.
                let router = emulator();
                if token_held {
                    import_github_token(&router).await;
                }
                let stem = format!("hook-unresolvable-{nth}");
                let project = seed(&router, form, &stem).await;

                let (status, body) = call(&router, operation, request(&project, extra)).await;
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
                let stated = body["message"].as_str().unwrap_or_default();
                assert!(
                    !stated.contains("webhook") && !stated.contains("Webhook"),
                    "case {case:?}: the refusal must not name a webhook, got {body}"
                );
                assert!(
                    !stated.contains("arn:") && !stated.contains(&project),
                    "case {case:?}: the sentence names neither the project nor its ARN, \
                     got {body}"
                );

                // The refusal persisted nothing: the name still resolves to no project,
                // and a held sibling keeps the webhook it was given.
                assert!(
                    resolves_to_nothing(&router, &project).await,
                    "case {case:?}: a refused call must mint no project"
                );
                if let (Some(held), _) = names(form, &stem) {
                    let (status, read) =
                        call(&router, "BatchGetProjects", json!({"names": [&held]})).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: BatchGetProjects {held:?}: expected 200, got {read}"
                    );
                    if form == Unresolvable::LetterCaseDiffersFromAHeldProject {
                        assert!(
                            read["projects"][0]["webhook"]["payloadUrl"].is_string(),
                            "case {case:?}: the held sibling keeps its webhook, got {read}"
                        );
                    }
                }
            }
        }
    }
}

/// The other half of the rule: the absent-webhook sentence is reserved for a project that
/// DOES resolve, so the two separately actionable conditions stay distinguishable.
///
/// The table spans the same request variations as above against a held project carrying no
/// webhook: `CreateWebhook` connects it, while the mutators that have to RESOLVE an
/// existing webhook in order to act on it report the WEBHOOK as the absent resource (or
/// refuse on the request itself) and never the project. `DeleteWebhook` is the one request
/// that resolves no webhook to act on it: a webhook that is not there is already the state
/// that call asks for, so recorded behaviour is the empty 200 and the project is left as it
/// was (the sentence's own doc states that exception).
#[tokio::test]
async fn a_held_project_with_no_webhook_reports_the_webhook() {
    for (nth, (operation, variation, extra)) in requests().iter().enumerate() {
        let case = format!("{operation} with {variation} on a held project with no webhook");
        let router = emulator();
        import_github_token(&router).await;
        let project = format!("hook-held-{nth}");
        create_project(&router, &project).await;

        let (status, body) = call(&router, operation, request(&project, extra)).await;
        if operation == &"CreateWebhook" && variation != &"a runner build type" {
            assert_eq!(
                status, 200,
                "case {case:?}: a held project takes a webhook, got {status} with {body}"
            );
            assert!(
                body["webhook"]["payloadUrl"].is_string(),
                "case {case:?}: the created webhook is reported, got {body}"
            );
            continue;
        }

        if operation == &"DeleteWebhook" {
            // The one request that resolves no webhook in order to act on it: a project
            // that carries none is already in the state a delete asks for, so the call is
            // an idempotent no-op — HTTP 200 with the output's no members, the project
            // still held and still carrying no webhook.
            assert_eq!(
                status, 200,
                "case {case:?}: a delete of a webhook that is not there is a no-op, got \
                 {status} with {body}"
            );
            assert_eq!(
                body,
                json!({}),
                "case {case:?}: the output models no members, got {body}"
            );
            let (read_status, read) =
                call(&router, "BatchGetProjects", json!({"names": [&project]})).await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: BatchGetProjects: expected 200, got {read}"
            );
            assert!(
                read["projectsNotFound"]
                    .as_array()
                    .is_some_and(|missing| missing.is_empty()),
                "case {case:?}: the no-op must leave the project held, got {read}"
            );
            assert!(
                read["projects"][0]["webhook"].is_null(),
                "case {case:?}: the project still carries no webhook, got {read}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        let stated = body["message"].as_str().unwrap_or_default().to_string();
        assert_ne!(
            body["message"],
            json!(state::PROJECT_NOT_FOUND_MESSAGE),
            "case {case:?}: a project that resolves is never reported absent, got {body}"
        );
        if operation == &"CreateWebhook" {
            // A runner build type is not one a repository-sourced project takes: the
            // refusal is about the request, not about either resource.
            assert_eq!(
                error_type(&body),
                "InvalidInputException",
                "case {case:?}: expected InvalidInputException, got {body}"
            );
            continue;
        }
        assert_eq!(
            error_type(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            stated,
            state::WEBHOOK_NOT_FOUND_MESSAGE,
            "case {case:?}: expected CodeBuild's absent-webhook sentence, got {body}"
        );
    }
}
